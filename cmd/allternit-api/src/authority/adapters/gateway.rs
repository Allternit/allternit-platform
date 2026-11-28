//! `gateway_approvals` (V199) through the kernel. `source_store` = `gateway`, `source_ref` = the
//! `gateway_approvals.id`; the old row keeps the kernel id in `kernel_request_id` and, once a
//! person approved, the approve token in `kernel_token` (both V300).
//!
//! - Create: `gateway_runner::create_approval` (every create site: held iMessage sends, phone
//!   holds, Platform text/call holds, channel posts, vendor approvals, consequential vendor sends,
//!   outbound calls) calls [`link`].
//! - Resolve: `gateway_runner::respond_approval` (the gateway route, the unified inbox `approval`
//!   and `held_send` items, the Platform twin) calls `adapters::resolve` before anything is
//!   forwarded or sent.
//! - Kernel inbox: [`write_back`] runs the same finish (vendor forward, row update, held send)
//!   without calling the kernel again.
//! - Consume: every place that spends an approved row (held iMessage send, phone hold, vendor
//!   send, Platform text/call) calls [`consume`], which redeems the kernel token.
//! - Closed without a person (phone hold expired or changed, vendor resolved it on its side):
//!   [`unlink`] withdraws it.
//!
//! Class: an outbound message (text, post, iMessage, call) is `client_message`; a phone action
//! touching a money app is `money`; a consequential vendor send is `network`; anything else
//! (vendor-defined actions, other phone actions) is held to `irreversible`.

use std::sync::Arc;

use allternit_authority::{ActionClass, AuthorityError, SourceStore};
use chrono::{Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use super::{Gate, Kernel, Open};
use crate::authority::check::Mode;
use crate::db::DbHandle;
use crate::AppState;

/// Gateway rows have no deadline of their own; a card can wait until the person is back.
const DEADLINE_DAYS: i64 = 7;
/// Ask the kernel for its longest token: a held send or retry can run a few minutes after approve.
const TOKEN_TTL_SECS: u64 = 900;

/// What the old store knows when it creates a row.
pub struct New<'a> {
    pub id: &'a str,
    pub owner: &'a str,
    pub thread_id: &'a str,
    pub bot_id: &'a str,
    pub authority: &'a str,
    pub action: &'a str,
    pub detail: &'a Value,
    pub correlation_id: Option<&'a str>,
}

/// The action class of a gateway approval.
pub fn class_for(authority: &str, action: &str, detail: &Value) -> ActionClass {
    let a = action.trim().to_lowercase();
    if authority == "vendor" {
        return ActionClass::Irreversible;
    }
    if a.starts_with("phone.") {
        let effect = detail.pointer("/phone/effect").and_then(Value::as_str).unwrap_or("");
        return match effect {
            "money_app" | "payment" | "pay" => ActionClass::Money,
            "send" | "message" | "call" => ActionClass::ClientMessage,
            _ => ActionClass::Irreversible,
        };
    }
    if a == crate::messages_mac::ACTION
        || ["post to ", "text ", "call ", "send ", "sms ", "email "].iter().any(|p| a.starts_with(p))
    {
        return if a == "send consequential work to the vendor" { ActionClass::Network } else { ActionClass::ClientMessage };
    }
    ActionClass::Irreversible
}

/// The exact action a person approves. The detail is bound by hash, not copied (it can carry the
/// message text and provenance).
pub fn open_for(n: &New<'_>) -> Open {
    let mut payload = serde_json::Map::new();
    payload.insert("store".into(), json!("gateway"));
    payload.insert("approval_id".into(), json!(n.id));
    payload.insert("thread_id".into(), json!(n.thread_id));
    payload.insert("bot_id".into(), json!(n.bot_id));
    payload.insert("authority".into(), json!(n.authority));
    payload.insert("action".into(), json!(n.action));
    payload.insert("correlation_id".into(), json!(n.correlation_id));
    payload.insert("detail_hash".into(), json!(super::content_hash(&n.detail.to_string())));
    let summary = n
        .detail
        .get("summary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| format!("{}: {}", if n.authority == "vendor" { "Vendor approval" } else { "Approve" }, n.action), str::to_string);
    Open {
        store: SourceStore::Gateway,
        source_ref: n.id.to_string(),
        owner: n.owner.to_string(),
        tenant: crate::authority::audit::DEFAULT_TENANT.to_string(),
        run_id: format!("gateway:{}", n.thread_id),
        principal: format!("gateway:{}/{}", n.bot_id, n.id),
        class: class_for(n.authority, n.action, n.detail),
        summary,
        payload,
        deadline: Utc::now() + Duration::days(DEADLINE_DAYS),
        token_ttl_secs: Some(TOKEN_TTL_SECS),
    }
}

/// The kernel id stored on the old row.
pub fn request_id(db: &DbHandle, id: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    conn.query_row("SELECT kernel_request_id FROM gateway_approvals WHERE id = ?1", params![id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
}

/// The new row as `create_approval` stored it (for a lazy link on first resolve).
fn load_new(db: &DbHandle, id: &str) -> Option<(String, String, String, String, String, String, Option<String>)> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT owner, thread_id, bot_id, authority, action, detail_json, correlation_id FROM gateway_approvals WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// Open the kernel row for a new gateway approval and store its id.
pub fn link(db: &DbHandle, n: &New<'_>) -> Result<Option<String>, AuthorityError> {
    let Some(rid) = super::open(db, open_for(n))? else { return Ok(None) };
    if let Ok(conn) = db.connect() {
        let _ = conn.execute("UPDATE gateway_approvals SET kernel_request_id = ?2 WHERE id = ?1", params![n.id, rid]);
    }
    Ok(Some(rid))
}

/// [`link`], logging instead of failing: create never blocks on the kernel (its resolve does).
pub fn link_logged(db: &DbHandle, n: &New<'_>) {
    if let Err(e) = link(db, n) {
        tracing::warn!(approval = %n.id, code = ?e.code, error = %e.message, "gateway approval: kernel request not opened");
    }
}

/// [`link`] for a row made before this adapter (or while the kernel was down).
pub fn link_existing(db: &DbHandle, id: &str) -> Result<Option<String>, AuthorityError> {
    let Some((owner, thread_id, bot_id, authority, action, detail, corr)) = load_new(db, id) else { return Ok(None) };
    let detail: Value = serde_json::from_str(&detail).unwrap_or(Value::Null);
    link(
        db,
        &New { id, owner: &owner, thread_id: &thread_id, bot_id: &bot_id, authority: &authority, action: &action, detail: &detail, correlation_id: corr.as_deref() },
    )
}

/// The kernel side of `respond_approval`, run before anything is forwarded or sent. `Err` means
/// refuse the old resolve.
pub async fn resolve(db: &DbHandle, gate: &Gate, id: &str, owner: &str, approve: bool, decided_by: &str) -> Result<(), AuthorityError> {
    let (db2, gate, id2, owner2) = (db.clone(), gate.clone(), id.to_string(), owner.to_string());
    let note = Some(format!("decided by {decided_by}"));
    let out = tokio::task::spawn_blocking(move || {
        let linked = request_id(&db2, &id2);
        let make_db = db2.clone();
        let make_id = id2.clone();
        super::resolve(&db2, &gate, linked, &owner2, approve, note, move || link_existing(&make_db, &make_id))
    })
    .await
    .map_err(|e| AuthorityError::kernel_unavailable(format!("kernel task: {e}")))??;
    match out {
        Kernel::Resolved { request_id, token } => store(db, id, &request_id, token.as_deref()),
        Kernel::AlreadySame { request_id } => store(db, id, &request_id, None),
        Kernel::Skipped => {}
    }
    Ok(())
}

/// Keep the kernel id (and the approve token) on the old row.
fn store(db: &DbHandle, id: &str, request_id: &str, token: Option<&str>) {
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "UPDATE gateway_approvals SET kernel_request_id = ?2, kernel_token = COALESCE(?3, kernel_token) WHERE id = ?1",
            params![id, request_id, token],
        );
    }
}

/// Spend the kernel token of an approved row, before the action runs. `Err(reason)` refuses the
/// action (enforce, or a kernel refusal with a token present in enforce); shadow logs and lets it
/// run. Called once per spend (the old row's `consumed` claim stays the old-client guard).
pub fn consume(db: &DbHandle, id: &str) -> Result<(), String> {
    let mode = super::mode();
    if mode == Mode::Off {
        return Ok(());
    }
    let row: Option<(String, Option<String>, Option<String>)> = db.connect().ok().and_then(|c| {
        c.query_row("SELECT owner, kernel_request_id, kernel_token FROM gateway_approvals WHERE id = ?1", params![id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .ok()
        .flatten()
    });
    // A Platform API runtime (`platform:` owner) has no Allternit person: its approvals are the
    // developer's decisions, recorded by cloud-api (`platform_approvals`, step 11b). Its kernel
    // row was withdrawn with that reason, so there is no token to spend here.
    if row.as_ref().is_some_and(|(owner, _, _)| owner.starts_with("platform:")) {
        return Ok(());
    }
    let Some((owner, Some(rid), Some(token))) = row else {
        if mode == Mode::Enforce {
            return Err("this approval has no kernel approval token; approve it again in the Allternit app".into());
        }
        tracing::warn!(approval = id, "gateway consume: no kernel token (shadow), action continues");
        return Ok(());
    };
    match super::redeem(db, &rid, &owner, &token) {
        Ok(_) => {
            // Spent: never present it again.
            if let Ok(c) = db.connect() {
                let _ = c.execute("UPDATE gateway_approvals SET kernel_token = NULL WHERE id = ?1", params![id]);
            }
            Ok(())
        }
        Err(e) if mode == Mode::Enforce => Err(e.message),
        Err(e) => {
            tracing::warn!(approval = id, code = ?e.code, error = %e.message, "gateway consume: kernel refused (shadow), action continues");
            Ok(())
        }
    }
}

/// The old row closed without a person (phone hold expired or changed, vendor resolved it on
/// its own side): withdraw its kernel row.
pub fn unlink(db: &DbHandle, id: &str, reason: &str) {
    super::withdraw(db, request_id(db, id).as_deref(), reason);
}

/// Kernel inbox resolved a gateway approval: finish it like `respond_approval` (vendor forward,
/// row update, held iMessage send), keep the token. Never calls the kernel.
pub async fn write_back(state: &Arc<AppState>, id: &str, request_id: &str, approve: bool, token: Option<String>, actor: &str) {
    store(&state.db, id, request_id, token.as_deref());
    let tx = crate::gateway_runner::transport(state);
    if let Err(e) = crate::gateway_runner::respond_from_kernel(&state.db, tx.as_ref(), actor, id, approve).await {
        tracing::warn!(approval = id, code = %e.code, error = %e.message, "gateway approval: kernel decision not written back");
    }
}
