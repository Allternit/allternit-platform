//! `cowork_approvals` (V169) through the kernel. `source_store` = `cowork`, `source_ref` = the
//! `cowork_approvals.id`. The old row keeps no column of its own (several tests and older
//! schemas build the table by hand): the kernel row is found by `(source_store, source_ref)`,
//! which V300 indexes.
//!
//! - Create: the inserts that hold a `DbHandle` call [`link_row`] after the row is written:
//!   coding-session permission asks (the agent-chat stream in `v1_routes`,
//!   `agent_session_routes::list_permissions`) and subscription MCP tasks. Team-rule routed asks
//!   (`team_rules::open_ask`, inside the connector gate's own connection) are linked on their
//!   first resolve. The kernel owner of a routed ask is its approver, so the kernel inbox shows
//!   it to the approver. Gateway cards (`source` = `gateway:*`) share their id with a gateway
//!   approval and get no second kernel row (the gateway adapter owns it).
//! - Resolve: `POST /cowork/approvals` (the ApprovalGate card) and `cowork_routes::decide_from_inbox`
//!   (the Needs-you inbox) gate the caller, claim the row, then call `adapters::resolve`; a kernel
//!   refusal reopens the row.
//! - Kernel inbox: [`write_back`] runs `cowork_routes::decide_from_kernel` (claim, relay to
//!   gizzi, routed-ask hook), no kernel call.
//! - Closed without a person decision (dismissed, withdrawn by its requester, approved by the
//!   Floor autopilot): [`unlink`] withdraws it with the reason on the receipt.

use std::sync::Arc;

use allternit_authority::{ActionClass, AuthorityError, SourceStore};
use chrono::{Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use super::Open;
use crate::db::DbHandle;
use crate::AppState;

/// Cards have no deadline of their own; a card can wait until the person is back.
const DEADLINE_DAYS: i64 = 7;

/// True for rows that belong to another adapter.
fn foreign(source: Option<&str>) -> bool {
    source.is_some_and(|s| s.starts_with("gateway:"))
}

/// The action class of a card, from what it asks.
pub fn class_for(source: Option<&str>, content: &Value) -> ActionClass {
    let permission = content
        .get("permission")
        .or_else(|| content.pointer("/details/actionType"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if content.get("kind").and_then(Value::as_str) == Some(crate::subscription_mcp::PREPARED_KIND) {
        return ActionClass::Network;
    }
    match permission.as_str() {
        "read" | "glob" | "grep" | "list" => ActionClass::Read,
        "edit" | "write" | "patch" | "multiedit" => ActionClass::FileWrite,
        "webfetch" | "websearch" | "subscription" | "mcp_app" => ActionClass::Network,
        "project_send" => ActionClass::ClientMessage,
        _ if source == Some("gizzi-permission") && permission.starts_with("external_directory") => ActionClass::FileWrite,
        // bash, task, unknown tools, team rule asks: the strictest class.
        _ => ActionClass::Irreversible,
    }
}

/// The exact card a person approves: its id, source, what it asks, and a hash of its content
/// (the content itself can hold secrets and stays in `cowork_approvals`).
pub fn open_for(id: &str, owner: &str, source: Option<&str>, content: &str) -> Open {
    let parsed: Value = serde_json::from_str(content).unwrap_or(Value::Null);
    let mut payload = serde_json::Map::new();
    payload.insert("store".into(), json!("cowork"));
    payload.insert("approval_id".into(), json!(id));
    payload.insert("source".into(), json!(source));
    payload.insert("content_hash".into(), json!(super::content_hash(content)));
    let what = parsed
        .get("title")
        .or_else(|| parsed.get("summary"))
        .or_else(|| parsed.get("permission"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| source.unwrap_or("approval").to_string());
    let session = parsed.get("sessionId").and_then(Value::as_str).filter(|s| !s.is_empty());
    Open {
        store: SourceStore::Cowork,
        source_ref: id.to_string(),
        owner: owner.to_string(),
        tenant: crate::authority::audit::DEFAULT_TENANT.to_string(),
        run_id: session.map_or_else(|| format!("cowork:{id}"), |s| format!("session:{s}")),
        principal: format!("cowork:{}/{id}", source.unwrap_or("card")),
        class: class_for(source, &parsed),
        summary: format!("Approve: {what}"),
        payload,
        deadline: Utc::now() + Duration::days(DEADLINE_DAYS),
        token_ttl_secs: None,
    }
}

/// The live (not withdrawn) kernel row behind a card, newest first.
pub fn request_id(db: &DbHandle, id: &str) -> Option<String> {
    super::request_for_live(db, SourceStore::Cowork, id)
}

/// A stored card as the kernel sees it: `(kernel owner, source, content)`. The kernel owner is the
/// person who answers it: the routed approver for a team-rule ask (V294 `approver_user_id`),
/// else the row's user. `None` owner for a global (NULL-user) row.
pub fn stored(conn: &rusqlite::Connection, id: &str) -> Option<(Option<String>, Option<String>, String)> {
    let routed = conn
        .query_row("SELECT user_id, source, content, approver_user_id FROM cowork_approvals WHERE id = ?1", params![id], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
        })
        .optional();
    match routed {
        Ok(row) => row.map(|(user, source, content, approver)| (approver.filter(|a| !a.is_empty()).or(user), source, content)),
        // Schemas before V294 (and hand-built test tables) have no approver column.
        Err(_) => conn
            .query_row("SELECT user_id, source, content FROM cowork_approvals WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .ok()
            .flatten(),
    }
}

/// The kernel owner of a card (see [`stored`]).
pub fn kernel_owner(conn: &rusqlite::Connection, id: &str) -> Option<String> {
    stored(conn, id).and_then(|(owner, _, _)| owner)
}

/// Open the kernel row for a card. `Ok(None)`: kernel off, a global row (no owner) or a gateway
/// card.
pub fn link(db: &DbHandle, id: &str, owner: Option<&str>, source: Option<&str>, content: &str) -> Result<Option<String>, AuthorityError> {
    let Some(owner) = owner.filter(|o| !o.is_empty()) else { return Ok(None) };
    if foreign(source) {
        return Ok(None);
    }
    if let Some(rid) = request_id(db, id) {
        return Ok(Some(rid));
    }
    super::open(db, open_for(id, owner, source, content))
}

/// Read a card back and [`link`] it, logging instead of failing (create never blocks on the
/// kernel; its resolve does). Called after every insert.
pub fn link_row(db: &DbHandle, id: &str) {
    if let Err(e) = link_existing(db, id) {
        tracing::warn!(approval = %id, code = ?e.code, error = %e.message, "cowork approval: kernel request not opened");
    }
}

/// [`link`] for a stored card.
pub fn link_existing(db: &DbHandle, id: &str) -> Result<Option<String>, AuthorityError> {
    let row = db.connect().ok().and_then(|c| stored(&c, id));
    match row {
        Some((owner, source, content)) => link(db, id, owner.as_deref(), source.as_deref(), &content),
        None => Ok(None),
    }
}

/// The card closed without a person's approve or deny: withdraw its kernel row.
pub fn unlink(db: &DbHandle, id: &str, reason: &str) {
    super::withdraw(db, request_id(db, id).as_deref(), reason);
}

/// Kernel inbox resolved a card: decide it like the card does (claim, relay, routed-ask hook).
/// Never calls the kernel.
pub async fn write_back(state: &Arc<AppState>, id: &str, approve: bool, actor: &str) {
    if let Err(e) = crate::cowork_routes::decide_from_kernel(state, id, approve, actor).await {
        tracing::warn!(approval = id, error = %e, "cowork approval: kernel decision not written back");
    }
}
