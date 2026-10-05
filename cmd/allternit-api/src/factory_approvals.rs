//! Allternit Factory approvals (SPEC §9 "Approvals", API.md "Approvals").
//!
//! When a Factory node waits on a person, the owner can approve or reject it
//! from any surface, and the first valid answer wins:
//!
//! | surface  | how it answers                                                      |
//! |----------|---------------------------------------------------------------------|
//! | app      | `POST /api/factory/approvals/:id/resolve` (session owner only)      |
//! | push     | `POST /api/factory/approvals/:id/push-action` (session owner + the  |
//! |          | one-time push code carried in the notification)                     |
//! | channels | Telegram / Slack buttons, or an SMS / email reply                   |
//! |          | `approve <node> <code>` from the owner's verified identity          |
//!
//! Where requests come from: the work ledger. A manual wait-gate whose node is
//! otherwise ready, or a judge `NEEDS_HUMAN` verdict, becomes one approval
//! (idempotent on dag + node + gate). A background sync ([`spawn_sync`]) and
//! every list call reconcile the table with the ledger, so an approval that
//! was answered somewhere else (Gizzi, the CLI) closes here too.
//!
//! The decision itself is always a Gate event, never a row here: a manual gate
//! is resolved with `resolve_node_wait_gate`, a judged node with
//! `judge_resolve`, both with the owner as the `user` actor and the surface,
//! channel and message id in the reason. The table only records the request,
//! where it was sent, and who answered first.
//!
//! High risk (money, deploy, client-message nodes by label, or a bot whose
//! autonomy policy for `factory` is `draft`/`ask`) goes to app and push only.
//!
//! One-time codes: 6 characters from an unambiguous alphabet, one per approval
//! and channel, stored as a hash only, single use, expire after 24h
//! (`ALLTERNIT_FACTORY_APPROVAL_CODE_TTL_HOURS`), burned after 5 wrong tries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use allternit_factory_engine::core::types::{Actor, ActorType, AllternitEvent, LedgerQuery};
use allternit_factory_engine::gate::gate::HumanDecision;
use allternit_factory_engine::judge::{pending_judge_needs, status as judge_status, events as judge_events};
use allternit_factory_engine::wait_gates::GateOutcome;
use allternit_factory_engine::work::needs_you::pending_manual_gates;
use allternit_factory_engine::work::project_dag;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{self, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, Mutex};

use crate::auth::AuthUser;
use crate::AppState;

/// Unambiguous code alphabet (no 0/O, 1/I/L).
pub const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
pub const CODE_LEN: usize = 6;
/// Wrong codes allowed per approval and channel before the code is burned.
pub const MAX_CODE_ATTEMPTS: i64 = 5;
pub const DEFAULT_CODE_TTL_HOURS: i64 = 24;
pub const DEFAULT_APPROVAL_TTL_HOURS: i64 = 24 * 7;
pub const DEFAULT_SYNC_SECS: u64 = 15;

pub const SURFACES: [&str; 6] = ["app", "push", "telegram", "slack", "sms", "email"];
pub const CHANNELS: [&str; 4] = ["telegram", "slack", "sms", "email"];

/// Labels that make a node high risk (also matched as the part after `:`,
/// e.g. `kind:deploy`). `risk:high` forces it.
const HIGH_RISK_LABELS: [(&str, &str); 16] = [
    ("money", "money"),
    ("payment", "money"),
    ("payments", "money"),
    ("invoice", "money"),
    ("billing", "money"),
    ("spend", "money"),
    ("purchase", "money"),
    ("refund", "money"),
    ("deploy", "deploy"),
    ("deployment", "deploy"),
    ("release", "deploy"),
    ("production", "deploy"),
    ("client-message", "client message"),
    ("client_message", "client message"),
    ("client-comms", "client message"),
    ("external-message", "client message"),
];

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse::<i64>().ok()).filter(|n| *n > 0).unwrap_or(default)
}

pub fn code_ttl() -> Duration {
    Duration::hours(env_i64("ALLTERNIT_FACTORY_APPROVAL_CODE_TTL_HOURS", DEFAULT_CODE_TTL_HOURS))
}

pub fn approval_ttl() -> Duration {
    Duration::hours(env_i64("ALLTERNIT_FACTORY_APPROVAL_TTL_HOURS", DEFAULT_APPROVAL_TTL_HOURS))
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

// ---------------------------------------------------------------- events

/// `approval.requested` / `approval.resolved` for the Factory events stream
/// (`{ type, at, data }`, API.md §3 Orchestration). The `/api/factory/events`
/// SSE merges this channel; bots also get a `bot_events` row.
static EVENTS: Lazy<broadcast::Sender<Value>> = Lazy::new(|| broadcast::channel(256).0);

pub fn subscribe() -> broadcast::Receiver<Value> {
    EVENTS.subscribe()
}

fn emit(state: &AppState, ty: &str, a: &Approval) {
    // `owner` lets SSE subscribers (the /api/factory/events proxy) send each
    // user only their own approvals; it is stripped before reaching clients.
    let evt = json!({ "type": ty, "at": now_rfc3339(), "owner": a.owner, "data": a.to_json() });
    let _ = EVENTS.send(evt);
    if let Some(bot) = a.bot_id.as_deref().filter(|b| !b.is_empty()) {
        crate::gateway_runner::led(&state.db, bot, "", None, ty, ("user", &a.owner), a.to_json(), Some(format!("factory:{ty}:{}", a.id)));
    }
}

// ---------------------------------------------------------------- model

#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub id: String,
    pub owner: String,
    pub dag_id: String,
    pub node_id: String,
    pub gate: String,
    pub bot_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub evidence_ref: Option<String>,
    pub risk: String,
    pub risk_reason: Option<String>,
    pub surfaces: Vec<String>,
    pub state: String,
    pub resolved_by: Option<String>,
    pub resolved_via: Option<String>,
    pub resolved_at: Option<String>,
    pub expires_at: String,
    pub created_at: String,
}

impl Approval {
    /// The API.md `Approval` shape (camelCase), plus `riskReason`.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "dagId": self.dag_id,
            "nodeId": self.node_id,
            "title": self.title,
            "summary": self.summary,
            "evidenceRef": self.evidence_ref,
            "risk": self.risk,
            "riskReason": self.risk_reason,
            "requestedAt": self.created_at,
            "surfaces": self.surfaces,
            "state": self.state,
            "resolvedBy": self.resolved_by,
            "resolvedVia": self.resolved_via,
            "resolvedAt": self.resolved_at,
            "expiresAt": self.expires_at,
        })
    }

    pub fn is_high_risk(&self) -> bool {
        self.risk == "high"
    }

    fn expired_at(&self, now: DateTime<Utc>) -> bool {
        parse_ts(&self.expires_at).is_some_and(|e| e <= now)
    }
}

const COLS: &str = "id, owner, dag_id, node_id, gate, bot_id, title, summary, evidence_ref, risk, risk_reason, surfaces_json, state, resolved_by, resolved_via, resolved_at, expires_at, created_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Approval> {
    let surfaces: String = r.get(11)?;
    Ok(Approval {
        id: r.get(0)?,
        owner: r.get(1)?,
        dag_id: r.get(2)?,
        node_id: r.get(3)?,
        gate: r.get(4)?,
        bot_id: r.get(5)?,
        title: r.get(6)?,
        summary: r.get(7)?,
        evidence_ref: r.get(8)?,
        risk: r.get(9)?,
        risk_reason: r.get(10)?,
        surfaces: serde_json::from_str(&surfaces).unwrap_or_else(|_| vec!["app".to_string()]),
        state: r.get(12)?,
        resolved_by: r.get(13)?,
        resolved_via: r.get(14)?,
        resolved_at: r.get(15)?,
        expires_at: r.get(16)?,
        created_at: r.get(17)?,
    })
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Approval>> {
    conn.query_row(&format!("SELECT {COLS} FROM factory_approvals WHERE id = ?1"), params![id], row).optional()
}

pub fn list(conn: &Connection, owner: &str, state: Option<&str>) -> rusqlite::Result<Vec<Approval>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM factory_approvals WHERE owner = ?1 AND (?2 IS NULL OR state = ?2) ORDER BY created_at DESC, id LIMIT 200"
    ))?;
    let out = st.query_map(params![owner, state], row)?.collect();
    out
}

/// Pending approvals for `owner` on `node_id` (any DAG): what a channel reply
/// `approve <node> <code>` can refer to.
pub fn pending_for_node(conn: &Connection, owner: &str, node_id: &str) -> rusqlite::Result<Vec<Approval>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM factory_approvals WHERE owner = ?1 AND node_id = ?2 AND state = 'pending' ORDER BY created_at DESC"
    ))?;
    let out = st.query_map(params![owner, node_id], row)?.collect();
    out
}

pub struct NewApproval {
    pub owner: String,
    pub dag_id: String,
    pub node_id: String,
    pub gate: String,
    pub bot_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub evidence_ref: Option<String>,
    pub risk: String,
    pub risk_reason: Option<String>,
}

/// Create the approval unless one already exists for (dag, node, gate).
/// Returns the new row, or `None` when it already existed.
pub fn insert_if_absent(conn: &Connection, n: &NewApproval, now: DateTime<Utc>) -> rusqlite::Result<Option<Approval>> {
    let id = format!("fa_{}", uuid::Uuid::new_v4().simple().to_string().chars().take(16).collect::<String>());
    let created = conn.execute(
        "INSERT OR IGNORE INTO factory_approvals (id, owner, dag_id, node_id, gate, bot_id, title, summary, evidence_ref, risk, risk_reason, surfaces_json, state, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, '[\"app\"]', 'pending', ?12, ?13)",
        params![id, n.owner, n.dag_id, n.node_id, n.gate, n.bot_id, n.title, n.summary, n.evidence_ref, n.risk, n.risk_reason, (now + approval_ttl()).to_rfc3339(), now.to_rfc3339()],
    )?;
    if created == 0 {
        return Ok(None);
    }
    get(conn, &id)
}

fn set_surfaces(conn: &Connection, id: &str, surfaces: &[String]) -> rusqlite::Result<()> {
    conn.execute("UPDATE factory_approvals SET surfaces_json = ?2 WHERE id = ?1", params![id, serde_json::to_string(surfaces).unwrap_or_default()])?;
    Ok(())
}

// ---------------------------------------------------------------- risk

/// `("high", reason)` for money, deploy and client-message nodes (by label),
/// or when the bot's autonomy policy for `factory` is `draft` or `ask`.
pub fn classify_risk(labels: &[String], autonomy_level: Option<&str>) -> (String, Option<String>) {
    for l in labels {
        let l = l.trim().to_lowercase();
        if l == "risk:high" {
            return ("high".into(), Some(high_risk_line("labeled high risk")));
        }
        let tail = l.rsplit(':').next().unwrap_or(&l);
        if let Some((_, what)) = HIGH_RISK_LABELS.iter().find(|(k, _)| *k == l || *k == tail) {
            return ("high".into(), Some(high_risk_line(&format!("a {what} node"))));
        }
    }
    if let Some(level) = autonomy_level.filter(|l| matches!(*l, "draft" | "ask")) {
        return ("high".into(), Some(high_risk_line(&format!("the bot's Factory autonomy is \"{level}\""))));
    }
    ("normal".into(), None)
}

fn high_risk_line(why: &str) -> String {
    format!("High risk ({why}): approve it in the Allternit app or from a push notification. Channel approvals are off for money, deploy and client-message work.")
}

// ---------------------------------------------------------------- codes

pub fn new_code() -> String {
    let mut rng = rand::thread_rng();
    (0..CODE_LEN).map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char).collect()
}

/// Hash of a one-time code, bound to the approval and channel it was issued for.
pub fn hash_code(approval_id: &str, channel: &str, code: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("allternit-factory-approval:v1:{approval_id}:{channel}:{}", code.trim().to_uppercase()).as_bytes());
    hex::encode(h.finalize())
}

/// Issue (or re-issue) the code for one approval and channel. Returns the
/// plaintext once; only the hash is stored.
pub fn issue_code(conn: &Connection, approval_id: &str, channel: &str, account_id: Option<&str>, now: DateTime<Utc>) -> rusqlite::Result<String> {
    let code = new_code();
    conn.execute(
        "INSERT OR REPLACE INTO factory_approval_codes (approval_id, channel, account_id, code_hash, attempts, expires_at, used_at, created_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, NULL, ?6)",
        params![approval_id, channel, account_id, hash_code(approval_id, channel, &code), (now + code_ttl()).to_rfc3339(), now.to_rfc3339()],
    )?;
    Ok(code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeCheck {
    Ok,
    NoCode,
    Wrong,
    Used,
    Expired,
    Burned,
}

impl CodeCheck {
    pub fn reason(self) -> &'static str {
        match self {
            CodeCheck::Ok => "ok",
            CodeCheck::NoCode => "no code was issued for this channel",
            CodeCheck::Wrong => "wrong code",
            CodeCheck::Used => "code already used",
            CodeCheck::Expired => "code expired",
            CodeCheck::Burned => "too many wrong codes; the code is burned",
        }
    }
}

/// Check a code without using it up (a wrong guess still counts an attempt).
pub fn check_code(conn: &Connection, approval_id: &str, channel: &str, code: &str, now: DateTime<Utc>) -> rusqlite::Result<CodeCheck> {
    let rowv: Option<(String, i64, String, Option<String>)> = conn
        .query_row(
            "SELECT code_hash, attempts, expires_at, used_at FROM factory_approval_codes WHERE approval_id = ?1 AND channel = ?2",
            params![approval_id, channel],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((hash, attempts, expires, used)) = rowv else { return Ok(CodeCheck::NoCode) };
    if used.is_some() {
        return Ok(CodeCheck::Used);
    }
    if attempts >= MAX_CODE_ATTEMPTS {
        return Ok(CodeCheck::Burned);
    }
    if parse_ts(&expires).map_or(true, |e| e <= now) {
        return Ok(CodeCheck::Expired);
    }
    if hash != hash_code(approval_id, channel, code) {
        conn.execute("UPDATE factory_approval_codes SET attempts = attempts + 1 WHERE approval_id = ?1 AND channel = ?2", params![approval_id, channel])?;
        return Ok(if attempts + 1 >= MAX_CODE_ATTEMPTS { CodeCheck::Burned } else { CodeCheck::Wrong });
    }
    Ok(CodeCheck::Ok)
}

/// Mark the code used. `false` when someone else used it first.
pub fn use_code(conn: &Connection, approval_id: &str, channel: &str, now: DateTime<Utc>) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE factory_approval_codes SET used_at = ?3 WHERE approval_id = ?1 AND channel = ?2 AND used_at IS NULL",
        params![approval_id, channel, now.to_rfc3339()],
    )? == 1)
}

fn expire_codes(conn: &Connection, approval_id: &str, now: DateTime<Utc>) -> rusqlite::Result<()> {
    conn.execute("UPDATE factory_approval_codes SET used_at = ?2 WHERE approval_id = ?1 AND used_at IS NULL", params![approval_id, now.to_rfc3339()])?;
    Ok(())
}

/// Log a refused answer with the reason (table + tracing).
#[allow(clippy::too_many_arguments)]
pub fn refuse(conn: &Connection, approval_id: Option<&str>, channel: &str, account_id: Option<&str>, sender: Option<&str>, message_id: Option<&str>, reason: &str) {
    tracing::warn!(approval = approval_id.unwrap_or("-"), channel, account = account_id.unwrap_or("-"), sender = sender.unwrap_or("-"), reason, "factory approval answer refused");
    let _ = conn.execute(
        "INSERT INTO factory_approval_refusals (id, approval_id, channel, account_id, sender, message_id, reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![uuid::Uuid::new_v4().to_string(), approval_id, channel, account_id, sender, message_id, reason, now_rfc3339()],
    );
}

// ---------------------------------------------------------------- reply text

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub approve: bool,
    pub node_id: String,
    pub code: String,
}

/// Parse a channel reply. The whole message must be exactly
/// `approve <node> <code>` or `reject <node> <code>` (case-insensitive verb),
/// so a quoted or forwarded copy with any other text never matches.
pub fn parse_reply(text: &str) -> Option<Reply> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() != 3 {
        return None;
    }
    let approve = match words[0].to_lowercase().as_str() {
        "approve" => true,
        "reject" => false,
        _ => return None,
    };
    let node = words[1];
    let code = words[2];
    let node_ok = !node.is_empty() && node.len() <= 128 && node.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    let code_ok = (6..=8).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric());
    (node_ok && code_ok).then(|| Reply { approve, node_id: node.to_string(), code: code.to_uppercase() })
}

/// The SMS / email request text.
pub fn reply_instructions(node_id: &str, code: &str) -> String {
    format!("Reply \"approve {node_id} {code}\" or \"reject {node_id} {code}\". The code works once and expires in {}h.", code_ttl().num_hours())
}

// ---------------------------------------------------------------- resolve

#[derive(Debug, Clone, Default)]
pub struct Provenance {
    /// `app` | `push` | `telegram` | `slack` | `sms` | `email`.
    pub surface: String,
    pub account_id: Option<String>,
    pub message_id: Option<String>,
    pub sender: Option<String>,
}

impl Provenance {
    pub fn app() -> Self {
        Provenance { surface: "app".into(), ..Default::default() }
    }
    fn to_json(&self) -> Value {
        json!({ "surface": self.surface, "channel": CHANNELS.contains(&self.surface.as_str()).then_some(&self.surface), "accountId": self.account_id, "messageId": self.message_id, "sender": self.sender })
    }
}

#[derive(Debug)]
pub enum ResolveError {
    NotFound,
    AlreadyResolved(Approval),
    Expired(Approval),
    /// High-risk approvals are app/push only.
    HighRisk(Approval),
    /// The Gate refused the decision (`code`, fact).
    Refused(String, String),
    Internal(String),
}

impl ResolveError {
    pub fn into_response(self) -> Response {
        match self {
            ResolveError::NotFound => api_err(StatusCode::NOT_FOUND, "not_found", "approval not found", "List approvals with GET /api/factory/approvals"),
            ResolveError::AlreadyResolved(a) => (
                StatusCode::CONFLICT,
                Json(json!({ "error": { "code": "refused", "fact": already_fact(&a), "action": "Nothing to do." }, "approval": a.to_json() })),
            )
                .into_response(),
            ResolveError::Expired(a) => (
                StatusCode::GONE,
                Json(json!({ "error": { "code": "refused", "fact": "this approval expired", "action": "Resolve the node with `gizzi workspace approve`" }, "approval": a.to_json() })),
            )
                .into_response(),
            ResolveError::HighRisk(_) => api_err(StatusCode::FORBIDDEN, "refused", "high-risk approvals are app and push only", "Approve it in the Allternit app"),
            ResolveError::Refused(code, fact) => api_err(StatusCode::CONFLICT, "refused", &format!("{code}: {fact}"), "Open the node to see its current state"),
            ResolveError::Internal(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", &e, "Try again"),
        }
    }
}

/// API.md 409 fact: "Already approved by <who> via <surface> at <time>".
fn already_fact(a: &Approval) -> String {
    let state = match a.state.as_str() {
        "approved" => "approved",
        "rejected" => "rejected",
        _ => "closed",
    };
    format!(
        "Already {state} by {} via {} at {}",
        a.resolved_by.as_deref().unwrap_or("someone"),
        a.resolved_via.as_deref().unwrap_or("another surface"),
        a.resolved_at.as_deref().unwrap_or("an earlier time")
    )
}

/// Serializes resolutions so the first valid answer wins (one process owns
/// the table; the Gate re-checks the node too).
static RESOLVE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Record the decision as a Gate event (owner as actor, surface as
/// provenance), mark the approval, expire the other codes, emit
/// `approval.resolved` and update the other surfaces.
pub async fn resolve(state: &Arc<AppState>, id: &str, approve: bool, actor_user: &str, prov: Provenance, note: Option<String>) -> Result<Approval, ResolveError> {
    let _guard = RESOLVE_LOCK.lock().await;
    let conn = state.db.connect().map_err(|e| ResolveError::Internal(e.to_string()))?;
    let a = get(&conn, id).map_err(|e| ResolveError::Internal(e.to_string()))?.ok_or(ResolveError::NotFound)?;
    if a.owner != actor_user {
        return Err(ResolveError::NotFound);
    }
    if a.state != "pending" {
        return Err(ResolveError::AlreadyResolved(a));
    }
    let now = Utc::now();
    if a.expired_at(now) {
        let _ = conn.execute("UPDATE factory_approvals SET state = 'expired', resolved_at = ?2, resolved_via = 'expiry' WHERE id = ?1 AND state = 'pending'", params![a.id, now.to_rfc3339()]);
        let _ = expire_codes(&conn, &a.id, now);
        let a = get(&conn, id).ok().flatten().unwrap_or(a);
        return Err(ResolveError::Expired(a));
    }
    if a.is_high_risk() && !matches!(prov.surface.as_str(), "app" | "push") {
        return Err(ResolveError::HighRisk(a));
    }
    drop(conn);

    let reason = format!(
        "{} via {}{}{} (factory approval {})",
        if approve { "approved" } else { "rejected" },
        prov.surface,
        prov.message_id.as_deref().map(|m| format!(", message {m}")).unwrap_or_default(),
        note.as_deref().filter(|n| !n.trim().is_empty()).map(|n| format!(": {}", n.trim())).unwrap_or_default(),
        a.id
    );
    if let Err((code, fact)) = gate_decide(state, &a, approve, actor_user, &reason).await {
        // Answered elsewhere first (Gizzi, CLI): close from the ledger and say so.
        if matches!(code.as_str(), "already_resolved" | "not_judged" | "gate_not_found" | "node_not_found") {
            let _ = sync_from_ledger(state).await;
            let conn = state.db.connect().map_err(|e| ResolveError::Internal(e.to_string()))?;
            if let Ok(Some(cur)) = get(&conn, id) {
                if cur.state != "pending" {
                    return Err(ResolveError::AlreadyResolved(cur));
                }
            }
        }
        return Err(ResolveError::Refused(code, fact));
    }

    let conn = state.db.connect().map_err(|e| ResolveError::Internal(e.to_string()))?;
    let now = Utc::now();
    conn.execute(
        "UPDATE factory_approvals SET state = ?2, resolved_by = ?3, resolved_via = ?4, resolved_at = ?5, provenance_json = ?6, note = ?7 WHERE id = ?1 AND state = 'pending'",
        params![a.id, if approve { "approved" } else { "rejected" }, actor_user, prov.surface, now.to_rfc3339(), prov.to_json().to_string(), note],
    )
    .map_err(|e| ResolveError::Internal(e.to_string()))?;
    let _ = expire_codes(&conn, &a.id, now);
    let done = get(&conn, &a.id).map_err(|e| ResolveError::Internal(e.to_string()))?.ok_or(ResolveError::NotFound)?;
    drop(conn);
    emit(state, "approval.resolved", &done);
    let st = state.clone();
    let announce = done.clone();
    tokio::spawn(async move { crate::factory_approvals_channels::announce_resolution(&st, &announce).await });
    Ok(done)
}

/// The Gate event for a decision. `Err((code, fact))` when refused.
async fn gate_decide(state: &Arc<AppState>, a: &Approval, approve: bool, actor_user: &str, reason: &str) -> Result<(), (String, String)> {
    let actor = Actor { r#type: ActorType::User, id: actor_user.to_string() };
    let rails = &state.rails;
    let scope = allternit_factory_engine::EventScope { dag_id: Some(a.dag_id.clone()), node_id: Some(a.node_id.clone()), ..Default::default() };
    allternit_factory_engine::policy::inject_policy(&rails.root_dir, &rails.ledger, Some(scope), "gateway").await.map_err(|e| ("policy".to_string(), e.to_string()))?;
    let res = if let Some(gate_id) = a.gate.strip_prefix("wait:") {
        let outcome = if approve { GateOutcome::Ok } else { GateOutcome::Failed };
        rails.gate.resolve_node_wait_gate(&a.dag_id, &a.node_id, gate_id, outcome, Some(actor), Some(reason.to_string())).await
    } else if a.gate.starts_with("judge:") {
        let decision = if approve { HumanDecision::Accomplished } else { HumanDecision::Abandon };
        rails.gate.judge_resolve(&a.dag_id, &a.node_id, decision, &actor, Some(reason)).await.map(|_| ())
    } else {
        return Err(("unknown_gate".into(), format!("approval gate {:?} is not a wait-gate or judge verdict", a.gate)));
    };
    res.map_err(|e| match allternit_factory_engine::GateError::from_anyhow(&e) {
        Some(g) => (g.code.clone(), g.reason.clone()),
        None => ("gate".into(), e.to_string()),
    })
}

// ---------------------------------------------------------------- ledger sync

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SyncReport {
    pub created: Vec<String>,
    pub closed: Vec<String>,
    pub skipped_no_owner: Vec<String>,
}

/// A node waiting on a person, read from the ledger.
struct Need {
    dag_id: String,
    node_id: String,
    gate: String,
    title: String,
    summary: String,
    evidence_ref: Option<String>,
    labels: Vec<String>,
    executor: Option<String>,
}

/// The Factory owner of a DAG: an `owner:<user>` node label, else the first
/// `user` actor in the DAG's events, else `ALLTERNIT_FACTORY_OWNER`, else the
/// account this runtime is paired to. `None` means nobody can be asked, and
/// no approval is created (logged, never guessed).
fn resolve_owner(events: &[&AllternitEvent], labels: &[String]) -> Option<String> {
    if let Some(o) = labels.iter().find_map(|l| l.strip_prefix("owner:")).map(str::trim).filter(|o| !o.is_empty()) {
        return Some(o.to_string());
    }
    if let Some(a) = events.iter().find(|e| e.actor.r#type == ActorType::User && !e.actor.id.trim().is_empty()) {
        return Some(a.actor.id.clone());
    }
    if let Some(o) = std::env::var("ALLTERNIT_FACTORY_OWNER").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Some(o);
    }
    crate::relay_auth::process_secret().paired_owner()
}

fn bot_for(conn: &Connection, owner: &str, executor: Option<&str>) -> Option<String> {
    let name = executor?.strip_prefix("bot:")?;
    conn.query_row(
        "SELECT id FROM agents WHERE user_id = ?1 AND (id = ?2 OR lower(name) = lower(?2)) ORDER BY (id = ?2) DESC LIMIT 1",
        params![owner, name],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn autonomy_level(state: &AppState, owner: &str, bot: Option<&str>) -> Option<String> {
    let bot = bot?;
    crate::autonomy::find_policy(&state.db, owner, bot, "factory", &[]).filter(|p| p.channel == "factory").map(|p| p.level)
}

static LAST_SYNC: Lazy<Mutex<Option<std::time::Instant>>> = Lazy::new(|| Mutex::new(None));
static NO_OWNER_LOGGED: Lazy<std::sync::Mutex<HashSet<String>>> = Lazy::new(|| std::sync::Mutex::new(HashSet::new()));

/// Reconcile the table with the ledger: create (and fan out) an approval for
/// every node now waiting on a person, and close pending approvals whose node
/// no longer waits (answered in Gizzi / the CLI, removed, or finished).
pub async fn sync_from_ledger(state: &Arc<AppState>) -> anyhow::Result<SyncReport> {
    *LAST_SYNC.lock().await = Some(std::time::Instant::now());
    let events = state.rails.ledger.query(LedgerQuery::default()).await?;
    let mut by_dag: BTreeMap<String, Vec<&AllternitEvent>> = BTreeMap::new();
    for e in &events {
        if let Some(d) = e.payload.get("dag_id").and_then(Value::as_str) {
            by_dag.entry(d.to_string()).or_default().push(e);
        }
    }
    let mut needs: Vec<Need> = Vec::new();
    let mut projected: HashMap<String, allternit_factory_engine::work::types::DagState> = HashMap::new();
    let mut project = |dag_id: &str| -> allternit_factory_engine::work::types::DagState {
        projected
            .entry(dag_id.to_string())
            .or_insert_with(|| {
                let evs: Vec<AllternitEvent> = by_dag.get(dag_id).map(|v| v.iter().map(|e| (*e).clone()).collect()).unwrap_or_default();
                project_dag(&evs, dag_id)
            })
            .clone()
    };
    for g in pending_manual_gates(&events).into_iter().filter(|g| g.deps_done) {
        let dag = project(&g.dag_id);
        let node = dag.nodes.get(&g.node_id);
        needs.push(Need {
            dag_id: g.dag_id.clone(),
            node_id: g.node_id.clone(),
            gate: format!("wait:{}", g.gate_id),
            title: g.node_title.clone(),
            summary: if g.description.trim().is_empty() { "Waiting for your go-ahead".into() } else { g.description.clone() },
            evidence_ref: node.and_then(|n| n.output.as_ref()).and_then(|o| serde_json::to_value(o).ok()).and_then(|v| v.get("path").and_then(Value::as_str).map(str::to_string)),
            labels: node.map(|n| n.labels.clone()).unwrap_or_default(),
            executor: node.and_then(|n| n.executor.clone()),
        });
    }
    for j in pending_judge_needs(&events) {
        let dag = project(&j.dag_id);
        let node = dag.nodes.get(&j.node_id);
        needs.push(Need {
            dag_id: j.dag_id.clone(),
            node_id: j.node_id.clone(),
            gate: format!("judge:{}", j.at),
            title: j.node_title.clone(),
            summary: format!("The checker needs a person{}: {}", j.category.as_deref().map(|c| format!(" ({c})")).unwrap_or_default(), j.detail),
            evidence_ref: j.wih_id.as_ref().map(|w| format!("wih:{w}")),
            labels: node.map(|n| n.labels.clone()).unwrap_or_default(),
            executor: node.and_then(|n| n.executor.clone()),
        });
    }

    let mut report = SyncReport::default();
    let live: HashSet<(String, String, String)> = needs.iter().map(|n| (n.dag_id.clone(), n.node_id.clone(), n.gate.clone())).collect();
    let mut fresh: Vec<Approval> = Vec::new();
    {
        let conn = state.db.connect()?;
        let now = Utc::now();
        for n in &needs {
            let dag_events = by_dag.get(&n.dag_id).cloned().unwrap_or_default();
            let Some(owner) = resolve_owner(&dag_events, &n.labels) else {
                let key = format!("{}/{}/{}", n.dag_id, n.node_id, n.gate);
                if NO_OWNER_LOGGED.lock().map(|mut s| s.insert(key.clone())).unwrap_or(false) {
                    tracing::warn!(dag = %n.dag_id, node = %n.node_id, "factory approval not created: no owner (add an owner:<user> label, run as a user, or set ALLTERNIT_FACTORY_OWNER)");
                }
                report.skipped_no_owner.push(key);
                continue;
            };
            let bot_id = bot_for(&conn, &owner, n.executor.as_deref());
            let (risk, risk_reason) = classify_risk(&n.labels, autonomy_level(state, &owner, bot_id.as_deref()).as_deref());
            let new = NewApproval { owner, dag_id: n.dag_id.clone(), node_id: n.node_id.clone(), gate: n.gate.clone(), bot_id, title: n.title.clone(), summary: n.summary.clone(), evidence_ref: n.evidence_ref.clone(), risk, risk_reason };
            if let Some(a) = insert_if_absent(&conn, &new, now)? {
                report.created.push(a.id.clone());
                fresh.push(a);
            }
        }

        // Close pending approvals the ledger no longer waits on.
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM factory_approvals WHERE state = 'pending'"))?;
        let pending: Vec<Approval> = st.query_map([], row)?.filter_map(Result::ok).collect();
        drop(st);
        for a in pending {
            if live.contains(&(a.dag_id.clone(), a.node_id.clone(), a.gate.clone())) {
                continue;
            }
            let dag = project(&a.dag_id);
            let (state_to, by) = closed_outcome(&a, &dag, by_dag.get(&a.dag_id).map(Vec::as_slice).unwrap_or(&[]));
            let n = conn.execute(
                "UPDATE factory_approvals SET state = ?2, resolved_by = ?3, resolved_via = 'engine', resolved_at = ?4 WHERE id = ?1 AND state = 'pending'",
                params![a.id, state_to, by, now.to_rfc3339()],
            )?;
            if n == 1 {
                let _ = expire_codes(&conn, &a.id, now);
                report.closed.push(a.id.clone());
                if let Ok(Some(done)) = get(&conn, &a.id) {
                    emit(state, "approval.resolved", &done);
                    let st = state.clone();
                    tokio::spawn(async move { crate::factory_approvals_channels::announce_resolution(&st, &done).await });
                }
            }
        }
    }
    for a in fresh {
        fan_out(state, a).await;
    }
    Ok(report)
}

/// How a pending approval ended when the ledger no longer waits on it.
fn closed_outcome(a: &Approval, dag: &allternit_factory_engine::work::types::DagState, events: &[&AllternitEvent]) -> (&'static str, Option<String>) {
    let node = dag.nodes.get(&a.node_id);
    if let Some(gate_id) = a.gate.strip_prefix("wait:") {
        if let Some(g) = node.and_then(|n| n.wait_gates.iter().find(|g| g.gate_id == gate_id)) {
            return match g.outcome {
                Some(GateOutcome::Ok) | Some(GateOutcome::Skipped) => ("approved", g.resolved_by.clone()),
                Some(GateOutcome::Failed) => ("rejected", g.resolved_by.clone()),
                None => ("expired", None),
            };
        }
        return ("expired", None);
    }
    let by = events
        .iter()
        .rev()
        .find(|e| e.r#type == judge_events::HUMAN_RESOLVED && e.payload.get("node_id").and_then(Value::as_str) == Some(a.node_id.as_str()))
        .and_then(|e| e.payload.get("by").and_then(Value::as_str).map(str::to_string));
    match node.map(|n| n.status.as_str()) {
        Some("DONE") => ("approved", by),
        Some("FAILED") => ("rejected", by),
        Some(s) if s == judge_status::NEEDS_HUMAN => ("expired", None),
        _ => ("expired", by),
    }
}

/// Send a new approval to push and the owner's channels, then record where
/// it actually went and emit `approval.requested`.
pub async fn fan_out(state: &Arc<AppState>, a: Approval) {
    let mut surfaces = vec!["app".to_string()];
    match crate::factory_approvals_push::send_request(state, &a).await {
        Ok(()) => surfaces.push("push".into()),
        Err(why) => tracing::info!(approval = %a.id, why, "factory approval: no push"),
    }
    if a.is_high_risk() {
        tracing::info!(approval = %a.id, "factory approval is high risk: app and push only");
    } else {
        surfaces.extend(crate::factory_approvals_channels::send_requests(state, &a).await);
    }
    let a = match state.db.connect() {
        Ok(conn) => {
            let _ = set_surfaces(&conn, &a.id, &surfaces);
            get(&conn, &a.id).ok().flatten().unwrap_or(a)
        }
        Err(_) => a,
    };
    emit(state, "approval.requested", &a);
}

/// Runs [`sync_from_ledger`] every `ALLTERNIT_FACTORY_APPROVALS_SYNC_SECS`
/// (default 15). It only reads the ledger and asks people; it never decides.
pub fn spawn_sync(state: Arc<AppState>) {
    let secs = env_i64("ALLTERNIT_FACTORY_APPROVALS_SYNC_SECS", DEFAULT_SYNC_SECS as i64) as u64;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(StdDuration::from_secs(secs));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = sync_from_ledger(&state).await {
                tracing::warn!(error = %e, "factory approvals sync failed");
            }
        }
    });
}

/// Sync on read unless a sync ran in the last few seconds.
async fn sync_if_stale(state: &Arc<AppState>) {
    let stale = LAST_SYNC.lock().await.map_or(true, |t| t.elapsed() > StdDuration::from_secs(3));
    if stale {
        if let Err(e) = sync_from_ledger(state).await {
            tracing::warn!(error = %e, "factory approvals sync on read failed");
        }
    }
}

// ---------------------------------------------------------------- inbox

/// Pending approvals for the "Needs you" inbox (`inbox_needs::collect`).
pub fn inbox_items(conn: &Connection, owner: &str) -> rusqlite::Result<Vec<Approval>> {
    let now = Utc::now();
    Ok(list(conn, owner, Some("pending"))?.into_iter().filter(|a| !a.expired_at(now)).collect())
}

// ---------------------------------------------------------------- routes

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/factory/approvals", routing::get(list_h))
        .route("/factory/approvals/:id/resolve", post(resolve_h))
        .route("/factory/approvals/:id/push-action", post(push_action_h))
        .merge(crate::factory_approvals_channels::identity_router())
}

pub fn api_err(status: StatusCode, code: &str, fact: &str, action: &str) -> Response {
    (status, Json(json!({ "error": { "code": code, "fact": fact, "action": action } }))).into_response()
}

#[derive(Deserialize)]
struct ListQuery {
    state: Option<String>,
}

async fn list_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<ListQuery>) -> Response {
    let filter = match q.state.as_deref().map(str::trim).filter(|s| !s.is_empty() && *s != "all") {
        None => None,
        Some(s @ ("pending" | "approved" | "rejected" | "expired")) => Some(s.to_string()),
        Some(other) => return api_err(StatusCode::BAD_REQUEST, "usage", &format!("state must be pending, approved, rejected, expired or all, not {other:?}"), "Fix the state filter"),
    };
    sync_if_stale(&state).await;
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", &e.to_string(), "Try again"),
    };
    // Lazily mark expired ones so the list never shows a dead pending row.
    let now = Utc::now();
    let _ = conn.execute("UPDATE factory_approvals SET state = 'expired', resolved_via = 'expiry', resolved_at = ?2 WHERE owner = ?1 AND state = 'pending' AND expires_at <= ?2", params![user.user_id, now.to_rfc3339()]);
    match list(&conn, &user.user_id, filter.as_deref()) {
        Ok(rows) => Json(json!({ "approvals": rows.iter().map(Approval::to_json).collect::<Vec<_>>() })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", &e.to_string(), "Try again"),
    }
}

#[derive(Deserialize)]
struct ResolveBody {
    decision: String,
    note: Option<String>,
    /// Push only: the one-time code from the notification.
    code: Option<String>,
}

fn decision(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "approve" => Some(true),
        "reject" => Some(false),
        _ => None,
    }
}

async fn resolve_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<ResolveBody>) -> Response {
    let Some(approve) = decision(&b.decision) else {
        return api_err(StatusCode::BAD_REQUEST, "usage", "decision must be approve or reject", "Send { decision: 'approve' | 'reject' }");
    };
    match resolve(&state, &id, approve, &user.user_id, Provenance::app(), b.note).await {
        Ok(a) => Json(a.to_json()).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Approve / Reject pressed on a push notification. The device must hold a
/// session for the owner, and the notification's one-time push code must
/// match (so a stale or copied notification can't answer).
async fn push_action_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<ResolveBody>) -> Response {
    let Some(approve) = decision(&b.decision) else {
        return api_err(StatusCode::BAD_REQUEST, "usage", "decision must be approve or reject", "Send { decision, code }");
    };
    let code = b.code.unwrap_or_default();
    {
        let conn = match state.db.connect() {
            Ok(c) => c,
            Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", &e.to_string(), "Try again"),
        };
        match get(&conn, &id) {
            Ok(Some(a)) if a.owner == user.user_id => {}
            Ok(Some(a)) => {
                refuse(&conn, Some(&a.id), "push", None, Some(&user.user_id), None, "push action from a session that is not the owner");
                return ResolveError::NotFound.into_response();
            }
            _ => return ResolveError::NotFound.into_response(),
        }
        match check_code(&conn, &id, "push", &code, Utc::now()) {
            Ok(CodeCheck::Ok) => {}
            Ok(c) => {
                refuse(&conn, Some(&id), "push", None, Some(&user.user_id), None, c.reason());
                return api_err(StatusCode::FORBIDDEN, "refused", c.reason(), "Open the approval in the app");
            }
            Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", &e.to_string(), "Try again"),
        }
    }
    match resolve(&state, &id, approve, &user.user_id, Provenance { surface: "push".into(), ..Default::default() }, b.note).await {
        Ok(a) => Json(a.to_json()).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::{HttpReq, HttpResp, HttpSend};
    use allternit_factory_engine::gate::gate::DagMutation;
    use allternit_factory_engine::wait_gates::WaitGateKind;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Tests that swap the shared HTTP seam or rely on sync timing run one at a time.
    static SERIAL: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Default)]
    struct FakeHttp {
        sent: std::sync::Mutex<Vec<HttpReq>>,
    }

    #[async_trait::async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            let body = if req.url.contains("api.telegram.org") {
                json!({ "ok": true, "result": { "message_id": 77 } })
            } else {
                json!({ "ok": true, "messageId": "m1", "ts": "1.2" })
            };
            self.sent.lock().unwrap().push(req);
            Ok(HttpResp { status: 200, body })
        }
    }

    fn user(id: &str) -> AuthUser {
        AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-fa-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        c.execute("INSERT INTO users (id, email, name) VALUES ('user-a', 'eoj@example.com', 'Eoj')", []).unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-a', 'user-a', 'builder', 'm', 'p', 1, '{}')", []).unwrap();
        state
    }

    /// A DAG with one node behind a manual wait-gate, owned by user-a.
    async fn waiting_node(state: &Arc<AppState>, labels: &[&str]) -> (String, String) {
        let gate = &state.rails.gate;
        allternit_factory_engine::policy::inject_policy(&state.rails.root_dir, &state.rails.ledger, None, "gateway").await.unwrap();
        let (_p, dag, node) = gate.plan_new("Ship the saved views", None).await.unwrap();
        let mut muts: Vec<DagMutation> = labels.iter().map(|l| DagMutation::AddLabel { node_id: node.clone(), label: l.to_string() }).collect();
        muts.push(DagMutation::AddLabel { node_id: node.clone(), label: "owner:user-a".into() });
        muts.push(DagMutation::AddWaitGate { node_id: node.clone(), gate_id: None, kind: WaitGateKind::Manual, description: Some("Ready to ship?".into()), params: Default::default() });
        gate.plan_refine(&dag, "needs a person", "user-a", muts).await.unwrap();
        (dag, node)
    }

    fn telegram_account(state: &AppState, owner_tg: &str) {
        let c = state.db.connect().unwrap();
        let sealed = crate::token_crypto::seal(&json!({ "botToken": "123:abc", "botUsername": "allternit_bot" }).to_string());
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, state, tg_owner_user_id) VALUES ('acct-tg', 'user-a', 'telegram', 'channel_oauth', ?1, 'CONNECTED', ?2)",
            params![sealed, owner_tg],
        )
        .unwrap();
    }

    async fn call(app: &Router, method: &str, uid: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path).header("content-type", "application/json").extension(user(uid)).body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn app(state: &Arc<AppState>) -> Router {
        Router::new().nest("/api", router()).with_state(state.clone())
    }

    fn tg_reply_update(from: i64, text: &str, extra: Value) -> Value {
        let mut msg = json!({ "message_id": 900, "from": { "id": from }, "chat": { "id": from, "type": "private" }, "text": text });
        if let (Some(m), Some(e)) = (msg.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        json!({ "update_id": 1, "message": msg })
    }

    fn sent_code(http: &FakeHttp) -> String {
        let sent = http.sent.lock().unwrap();
        let req = sent.iter().find(|r| r.url.ends_with("/sendMessage") && r.body.get("reply_markup").is_some()).expect("telegram request");
        let data = req.body.pointer("/reply_markup/inline_keyboard/0/0/callback_data").and_then(Value::as_str).unwrap();
        data.rsplit(':').next().unwrap().to_string()
    }

    fn refusals(state: &AppState) -> Vec<String> {
        let c = state.db.connect().unwrap();
        let mut q = c.prepare("SELECT reason FROM factory_approval_refusals ORDER BY created_at").unwrap();
        let out = q.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect();
        out
    }

    // ------------------------------------------------------------ pure

    #[test]
    fn reply_must_be_exactly_the_answer() {
        assert_eq!(parse_reply("approve n_02 abc234"), Some(Reply { approve: true, node_id: "n_02".into(), code: "ABC234".into() }));
        assert_eq!(parse_reply("  Reject n_02 ABC234 ").map(|r| r.approve), Some(false));
        assert_eq!(parse_reply("> approve n_02 ABC234"), None, "quoted");
        assert_eq!(parse_reply("approve n_02 ABC234 thanks"), None, "extra text");
        assert_eq!(parse_reply("Fwd: approve n_02 ABC234"), None);
        assert_eq!(parse_reply("approve n_02 AB"), None, "short code");
        assert_eq!(parse_reply("approve n_02;drop ABC234"), None);
    }

    #[test]
    fn high_risk_by_label_or_autonomy() {
        assert_eq!(classify_risk(&["deploy".into()], None).0, "high");
        assert_eq!(classify_risk(&["kind:payment".into()], None).0, "high");
        assert_eq!(classify_risk(&["client-message".into()], None).0, "high");
        assert_eq!(classify_risk(&["risk:high".into()], None).0, "high");
        assert_eq!(classify_risk(&["docs".into()], Some("ask")).0, "high");
        assert_eq!(classify_risk(&["docs".into()], Some("limits")), ("normal".into(), None));
        assert!(classify_risk(&["deploy".into()], None).1.unwrap().contains("app or from a push"));
    }

    #[tokio::test]
    async fn codes_are_hashed_single_use_expiring_and_burn() {
        let state = setup().await;
        let c = state.db.connect().unwrap();
        let now = Utc::now();
        let code = issue_code(&c, "fa_1", "sms", Some("acct"), now).unwrap();
        assert!((6..=8).contains(&code.len()) && code.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        let stored: String = c.query_row("SELECT code_hash FROM factory_approval_codes WHERE approval_id = 'fa_1'", [], |r| r.get(0)).unwrap();
        assert_ne!(stored, code);
        assert!(!stored.contains(&code), "only a hash is stored");
        assert_eq!(stored, hash_code("fa_1", "sms", &code.to_lowercase()));
        // Bound to the channel it was issued for.
        assert_eq!(check_code(&c, "fa_1", "email", &code, now).unwrap(), CodeCheck::NoCode);
        assert_eq!(check_code(&c, "fa_1", "sms", &code, now).unwrap(), CodeCheck::Ok);
        assert!(use_code(&c, "fa_1", "sms", now).unwrap());
        assert!(!use_code(&c, "fa_1", "sms", now).unwrap(), "single use");
        assert_eq!(check_code(&c, "fa_1", "sms", &code, now).unwrap(), CodeCheck::Used);
        // Expiry.
        let code = issue_code(&c, "fa_2", "sms", None, now).unwrap();
        assert_eq!(check_code(&c, "fa_2", "sms", &code, now + code_ttl() + Duration::seconds(1)).unwrap(), CodeCheck::Expired);
        // Burned after too many wrong guesses, even with the right code afterwards.
        let code = issue_code(&c, "fa_3", "sms", None, now).unwrap();
        for _ in 0..MAX_CODE_ATTEMPTS - 1 {
            assert_eq!(check_code(&c, "fa_3", "sms", "ZZZZZZ", now).unwrap(), CodeCheck::Wrong);
        }
        assert_eq!(check_code(&c, "fa_3", "sms", "ZZZZZZ", now).unwrap(), CodeCheck::Burned);
        assert_eq!(check_code(&c, "fa_3", "sms", &code, now).unwrap(), CodeCheck::Burned);
    }

    // ------------------------------------------------------------ ledger + routes

    #[tokio::test]
    async fn wait_gate_becomes_one_approval_and_app_resolve_is_a_gate_event() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let (dag, node) = waiting_node(&state, &[]).await;
        let app = app(&state);
        // The read-side sync throttle is process-wide; another test's state may have just synced.
        *LAST_SYNC.lock().await = None;

        let (st, body) = call(&app, "GET", "user-a", "/api/factory/approvals?state=pending", None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let list = body["approvals"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        let a = &list[0];
        assert_eq!((a["dagId"].as_str(), a["nodeId"].as_str(), a["risk"].as_str()), (Some(dag.as_str()), Some(node.as_str()), Some("normal")));
        assert_eq!(a["surfaces"], json!(["app"]));
        // Idempotent: another sync makes nothing new.
        assert!(sync_from_ledger(&state).await.unwrap().created.is_empty());
        // Another user sees nothing and can't resolve it.
        let (_, other) = call(&app, "GET", "user-b", "/api/factory/approvals", None).await;
        assert_eq!(other["approvals"], json!([]));
        let id = a["id"].as_str().unwrap().to_string();
        let (st, _) = call(&app, "POST", "user-b", &format!("/api/factory/approvals/{id}/resolve"), Some(json!({ "decision": "approve" }))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = call(&app, "POST", "user-a", &format!("/api/factory/approvals/{id}/resolve"), Some(json!({ "decision": "maybe" }))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        let mut events = subscribe();
        let (st, body) = call(&app, "POST", "user-a", &format!("/api/factory/approvals/{id}/resolve"), Some(json!({ "decision": "approve", "note": "looks right" }))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "approved");
        assert_eq!(body["resolvedBy"], "user-a");
        assert_eq!(body["resolvedVia"], "app");
        let evt = events.recv().await.unwrap();
        assert_eq!(evt["type"], "approval.resolved");
        assert_eq!(evt["owner"], "user-a", "SSE subscribers filter on the owner");

        // The Gate recorded it: the gate is resolved ok by the owner as a user.
        let all = state.rails.ledger.query(LedgerQuery::default()).await.unwrap();
        let resolved = all.iter().find(|e| e.r#type == "DagNodeWaitGateResolved").expect("gate event");
        assert_eq!(resolved.actor.r#type, ActorType::User);
        assert_eq!(resolved.actor.id, "user-a");
        assert!(resolved.payload["reason"].as_str().unwrap().contains("via app"));

        // First answer won: a second one is a conflict that says who and where.
        let (st, body) = call(&app, "POST", "user-a", &format!("/api/factory/approvals/{id}/resolve"), Some(json!({ "decision": "reject" }))).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(body["error"]["fact"].as_str().unwrap().starts_with("Already approved by user-a via app at "));
        // The node no longer waits, and sync doesn't reopen it.
        assert!(sync_from_ledger(&state).await.unwrap().created.is_empty());
    }

    #[tokio::test]
    async fn expired_approval_is_gone() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        c.execute("UPDATE factory_approvals SET expires_at = ?2 WHERE id = ?1", params![a.id, (Utc::now() - Duration::minutes(1)).to_rfc3339()]).unwrap();
        let (st, body) = call(&app(&state), "POST", "user-a", &format!("/api/factory/approvals/{}/resolve", a.id), Some(json!({ "decision": "approve" }))).await;
        assert_eq!(st, StatusCode::GONE, "{body}");
        assert_eq!(body["approval"]["state"], "expired");
        assert!(inbox_items(&c, "user-a").unwrap().is_empty());
    }

    #[tokio::test]
    async fn answered_in_the_cli_closes_here() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let (dag, node) = waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        let gate_id = a.gate.strip_prefix("wait:").unwrap();
        state.rails.gate.resolve_node_wait_gate(&dag, &node, gate_id, GateOutcome::Ok, Some(Actor { r#type: ActorType::User, id: "user-a".into() }), Some("cli".into())).await.unwrap();
        let r = sync_from_ledger(&state).await.unwrap();
        assert_eq!(r.closed, vec![a.id.clone()]);
        let a = get(&c, &a.id).unwrap().unwrap();
        assert_eq!((a.state.as_str(), a.resolved_via.as_deref()), ("approved", Some("engine")));
    }

    #[tokio::test]
    async fn telegram_owner_reply_approves_and_others_are_refused() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let http = Arc::new(FakeHttp::default());
        crate::factory_approvals_channels::set_http(Some(http.clone()));
        telegram_account(&state, "4242");
        let (_dag, node) = waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        assert_eq!(a.surfaces, vec!["app".to_string(), "telegram".to_string()]);
        let code = sent_code(&http);
        let req_text = http.sent.lock().unwrap()[0].body["text"].as_str().unwrap().to_string();
        assert!(req_text.contains(&format!("\"approve {node} {code}\" or \"reject {node} {code}\"")), "{req_text}");
        let acct = crate::channel_transports::accounts(&state.db, "telegram", Some("acct-tg")).remove(0);

        // Wrong sender: refused, logged, no reply.
        crate::factory_approvals_channels::telegram_update(&state, &acct, &tg_reply_update(999, &format!("approve {node} {code}"), json!({}))).await;
        // Forwarded by the owner: refused.
        crate::factory_approvals_channels::telegram_update(&state, &acct, &tg_reply_update(4242, &format!("approve {node} {code}"), json!({ "forward_origin": { "type": "user" } }))).await;
        // Wrong code from the owner: refused.
        crate::factory_approvals_channels::telegram_update(&state, &acct, &tg_reply_update(4242, &format!("approve {node} ZZZZZZ"), json!({}))).await;
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "pending");
        let reasons = refusals(&state);
        assert!(reasons.iter().any(|r| r == "sender is not the verified owner"), "{reasons:?}");
        assert!(reasons.iter().any(|r| r == "forwarded message"), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("wrong code")), "{reasons:?}");

        // The owner's own reply wins.
        crate::factory_approvals_channels::telegram_update(&state, &acct, &tg_reply_update(4242, &format!("approve {node} {code}"), json!({}))).await;
        let done = get(&c, &a.id).unwrap().unwrap();
        assert_eq!((done.state.as_str(), done.resolved_via.as_deref()), ("approved", Some("telegram")));
        let prov: String = c.query_row("SELECT provenance_json FROM factory_approvals WHERE id = ?1", params![a.id], |r| r.get(0)).unwrap();
        assert!(prov.contains("\"messageId\":\"900\""), "{prov}");
        // The code is single use: replaying the same message is refused.
        crate::factory_approvals_channels::telegram_update(&state, &acct, &tg_reply_update(4242, &format!("reject {node} {code}"), json!({}))).await;
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "approved");
        // The request was edited to say who answered where.
        crate::factory_approvals_channels::announce_resolution(&state, &done).await;
        let edited = http.sent.lock().unwrap().iter().any(|r| r.url.ends_with("/editMessageText") && r.body["text"].as_str().unwrap_or("").contains("Approved by Eoj in Telegram"));
        assert!(edited);
        crate::factory_approvals_channels::set_http(None);
    }

    #[tokio::test]
    async fn telegram_button_press_from_owner_rejects() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let http = Arc::new(FakeHttp::default());
        crate::factory_approvals_channels::set_http(Some(http.clone()));
        telegram_account(&state, "4242");
        waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let code = sent_code(&http);
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        let acct = crate::channel_transports::accounts(&state.db, "telegram", Some("acct-tg")).remove(0);
        let press = |from: i64| json!({ "update_id": 2, "callback_query": { "id": "cq1", "from": { "id": from }, "data": format!("fa:{}:r:{code}", a.id), "message": { "message_id": 77 } } });
        assert!(crate::factory_approvals_channels::telegram_is_factory(&press(1)));
        crate::factory_approvals_channels::telegram_update(&state, &acct, &press(1)).await;
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "pending");
        crate::factory_approvals_channels::telegram_update(&state, &acct, &press(4242)).await;
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "rejected");
        assert!(http.sent.lock().unwrap().iter().any(|r| r.url.ends_with("/answerCallbackQuery")));
        crate::factory_approvals_channels::set_http(None);
    }

    #[tokio::test]
    async fn high_risk_goes_to_app_only_and_channels_are_refused() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let http = Arc::new(FakeHttp::default());
        crate::factory_approvals_channels::set_http(Some(http.clone()));
        telegram_account(&state, "4242");
        let (_dag, node) = waiting_node(&state, &["deploy"]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        assert_eq!(a.risk, "high");
        assert_eq!(a.surfaces, vec!["app".to_string()]);
        assert!(a.to_json()["riskReason"].as_str().unwrap().contains("deploy"));
        assert!(http.sent.lock().unwrap().is_empty(), "no channel request for high risk");
        // Even a guessed code on a channel is refused; the app still works.
        let acct = crate::channel_transports::accounts(&state.db, "telegram", Some("acct-tg")).remove(0);
        crate::factory_approvals_channels::telegram_update(&state, &acct, &tg_reply_update(4242, &format!("approve {node} ABCDEF"), json!({}))).await;
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "pending");
        let err = resolve(&state, &a.id, true, "user-a", Provenance { surface: "sms".into(), ..Default::default() }, None).await.unwrap_err();
        assert!(matches!(err, ResolveError::HighRisk(_)));
        let done = resolve(&state, &a.id, true, "user-a", Provenance::app(), None).await.unwrap();
        assert_eq!(done.state, "approved");
        crate::factory_approvals_channels::set_http(None);
    }

    #[tokio::test]
    async fn slack_verify_pairs_the_owner_then_reply_approves() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let c = state.db.connect().unwrap();
        let sealed = crate::token_crypto::seal(&json!({ "teamId": "T1", "teamName": "Acme", "sharedApp": true }).to_string());
        c.execute("INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, state) VALUES ('acct-sl', 'user-a', 'slack', 'channel_oauth', ?1, 'CONNECTED')", params![sealed]).unwrap();
        let app = app(&state);
        let (st, body) = call(&app, "POST", "user-a", "/api/factory/approvals/identities", Some(json!({ "accountId": "acct-sl" }))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let vcode = body["code"].as_str().unwrap().to_string();
        // Another user can't start a verify for this account.
        let (st, _) = call(&app, "POST", "user-b", "/api/factory/approvals/identities", Some(json!({ "accountId": "acct-sl" }))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let acct = crate::channel_transports::accounts(&state.db, "slack", Some("acct-sl")).remove(0);
        let ans = |sender: &'static str, text: String| crate::factory_approvals_channels::Answer { channel: "slack", account: &acct, sender: Some(sender), text: Box::leak(text.into_boxed_str()), message_id: "1.1", forwarded: false };
        let r = crate::factory_approvals_channels::handle_answer(&state, ans("U_OWNER", format!("verify {vcode}"))).await;
        assert!(r.unwrap().starts_with("Verified"));
        let (_, ids) = call(&app, "GET", "user-a", "/api/factory/approvals/identities", None).await;
        assert!(ids["identities"].as_array().unwrap().iter().any(|i| i["channel"] == "slack" && i["identity"] == "U_OWNER"));

        // An approval with a slack code (the send itself goes through the cloud; issue directly here).
        let (_dag, node) = waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        let code = issue_code(&c, &a.id, "slack", Some("acct-sl"), Utc::now()).unwrap();
        let r = crate::factory_approvals_channels::handle_answer(&state, ans("U_OTHER", format!("approve {node} {code}"))).await;
        assert_eq!(r.as_deref(), Some(""), "a stranger gets no reply");
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "pending");
        let r = crate::factory_approvals_channels::handle_answer(&state, ans("U_OWNER", format!("approve {node} {code}"))).await;
        assert!(r.unwrap().starts_with("Approved"));
        assert_eq!(get(&c, &a.id).unwrap().unwrap().resolved_via.as_deref(), Some("slack"));
    }

    #[tokio::test]
    async fn email_reply_from_owner_only_and_never_forwarded() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        let (_dag, node) = waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        let code = issue_code(&c, &a.id, "email", Some("bot-a"), Utc::now()).unwrap();
        let body = format!("approve {node} {code}\n\nOn Mon, Allternit wrote:\n> Allternit Factory: needs your OK");
        let fa = crate::factory_approvals_channels::email_answer;
        // Not an answer: falls through to the bot.
        assert!(!fa(&state, "user-a", "bot-a", "eoj@example.com", Some("hi"), "hello there", "m0").await);
        // Someone else, and a forward: consumed and refused.
        assert!(fa(&state, "user-a", "bot-a", "Mallory <mallory@example.com>", Some("Re: Approve"), &body, "m1").await);
        assert!(fa(&state, "user-a", "bot-a", "Eoj <eoj@example.com>", Some("Fwd: Approve"), &body, "m2").await);
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "pending");
        let reasons = refusals(&state);
        assert!(reasons.iter().any(|r| r == "sender is not the owner's account email"), "{reasons:?}");
        assert!(reasons.iter().any(|r| r == "forwarded message"), "{reasons:?}");
        assert!(fa(&state, "user-a", "bot-a", "Eoj <EOJ@example.com>", Some("Re: Approve"), &body, "m3").await);
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, "approved");
    }

    #[tokio::test]
    async fn push_action_needs_owner_session_and_push_code() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let a = list(&c, "user-a", Some("pending")).unwrap().remove(0);
        let code = issue_code(&c, &a.id, "push", None, Utc::now()).unwrap();
        let app = app(&state);
        let path = format!("/api/factory/approvals/{}/push-action", a.id);
        let (st, _) = call(&app, "POST", "user-b", &path, Some(json!({ "decision": "approve", "code": code }))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = call(&app, "POST", "user-a", &path, Some(json!({ "decision": "approve", "code": "ZZZZZZ" }))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, body) = call(&app, "POST", "user-a", &path, Some(json!({ "decision": "approve", "code": code }))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["resolvedVia"], "push");
        let body = crate::factory_approvals_push::notify_body(&a, "rt_1", "ABC234");
        assert_eq!(body["data"]["approvalId"], a.id.as_str());
        assert_eq!(body["runtimeId"], "rt_1");
    }

    #[tokio::test]
    async fn pending_approvals_show_in_needs_you_and_approve_there() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        waiting_node(&state, &[]).await;
        sync_from_ledger(&state).await.unwrap();
        let c = state.db.connect().unwrap();
        let items = crate::inbox_needs::collect(&c, "user-a", 30).unwrap();
        let it = items.iter().find(|i| i["kind"] == "factory_approval").expect("needs-you item");
        assert_eq!(it["actions"][0], "approve");
        let id = it["id"].as_str().unwrap().to_string();
        let inbox = Router::new().nest("/api/v1", crate::inbox_needs::needs_router()).with_state(state.clone());
        let (st, body) = call(&inbox, "POST", "user-a", &format!("/api/v1/inbox/{id}/approve"), Some(json!({ "decision": "deny" }))).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["result"]["state"], "rejected");
        assert!(crate::inbox_needs::collect(&c, "user-a", 30).unwrap().iter().all(|i| i["kind"] != "factory_approval"));
    }
}
