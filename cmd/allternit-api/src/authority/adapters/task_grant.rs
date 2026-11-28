//! `task_grants` (V276) through the kernel. A grant is a standing permission ("you may send and
//! upload for this task"), so it maps to one kernel approval that stays `approved` while the
//! grant is live, and every use is a kernel `check` that names the grant as its token source.
//! `source_store` = `task_grant`, `source_ref` = the grant id; the grant row keeps no column (the
//! kernel row is found by `(source_store, source_ref)`, newest first).
//!
//! - Create (`task_grants::create`): [`link`] opens an `approval.request` for the grant's exact
//!   action set. A `card` grant is pressed by a person (every creator checks the person's session
//!   proof first: `POST /task-grants`, "Allow for this task" on a card, "Approve plan and allow
//!   its sends and uploads"), so it is resolved at once as that person: one `request` and one
//!   `approve` receipt. A `words` grant comes from gizzi's `grant_task_permission` tool (the
//!   owner's own words, read by an agent), so the kernel row stays OPEN until the person approves
//!   it in the kernel inbox; shadow lets it work meanwhile, enforce does not. When a live grant
//!   gains actions, a new kernel row covers the full set (the newest row is the one counted).
//! - Live check (`task_grants::live_grant_for`, used by autonomy, the channel gateway, ACI and
//!   the phone guard): [`admit`] writes a `check` receipt (`token_source: task_grant`) and
//!   allows only while the newest kernel row is approved, covers the action and is before its
//!   deadline. Shadow writes the receipt and allows.
//! - Revoke (`task_grants::revoke`, `revoke_for_thread`): [`revoke`] closes the kernel row
//!   (`approved` too), with a `withdraw` receipt marked `revoked`.
//! - Kernel inbox: [`write_back`]; approve makes a words grant count, deny revokes the grant.

use allternit_authority::{ActionClass, AuthorityError, Caller, CallerKind, ReceiptKind, SourceStore};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use super::Open;
use crate::authority::approvals::{Decision, ResolveBody};
use crate::authority::check::{Mode, CHECK_SCHEMA_ID};
use crate::db::DbHandle;
use crate::task_grants::Grant;

fn rank(c: ActionClass) -> u8 {
    match c {
        ActionClass::Irreversible => 5,
        ActionClass::Money => 4,
        ActionClass::ClientMessage => 3,
        ActionClass::Network => 2,
        ActionClass::FileWrite => 1,
        _ => 0,
    }
}

/// The class of one granted action.
fn class_of(action: &str) -> ActionClass {
    match action {
        "send" | "call" => ActionClass::ClientMessage,
        "upload" | "webfetch" | "websearch" => ActionClass::Network,
        "edit" | "write" | "patch" | "multiedit" => ActionClass::FileWrite,
        "read" | "glob" | "grep" | "list" => ActionClass::Read,
        // submit, booking, bash and anything else: the strictest class.
        _ => ActionClass::Irreversible,
    }
}

/// The strictest class among the granted actions.
pub fn class_for(actions: &[String]) -> ActionClass {
    actions.iter().map(|a| class_of(a)).max_by_key(|c| rank(*c)).unwrap_or(ActionClass::Irreversible)
}

/// The standing permission a person grants: these actions on this thread for this bot.
pub fn open_for(g: &Grant) -> Open {
    let mut actions = g.actions.clone();
    actions.sort();
    let mut payload = serde_json::Map::new();
    payload.insert("store".into(), json!("task_grant"));
    payload.insert("grant_id".into(), json!(g.id));
    payload.insert("thread_id".into(), json!(g.thread_id));
    payload.insert("bot_id".into(), json!(g.bot_id));
    payload.insert("actions".into(), json!(actions));
    payload.insert("source".into(), json!(g.source));
    payload.insert("source_ref".into(), json!(g.source_ref));
    Open {
        store: SourceStore::TaskGrant,
        source_ref: g.id.clone(),
        owner: g.owner.clone(),
        tenant: crate::authority::audit::DEFAULT_TENANT.to_string(),
        run_id: format!("thread:{}", g.thread_id),
        principal: format!("task_grant:{}/{}", if g.bot_id.is_empty() { "any" } else { g.bot_id.as_str() }, g.id),
        class: class_for(&actions),
        summary: format!("Allow for this task: {}", actions.join(", ")),
        payload,
        deadline: DateTime::parse_from_rfc3339(&g.expires_at).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now() + chrono::Duration::days(crate::task_grants::TTL_DAYS)),
        token_ttl_secs: None,
    }
}

/// The newest kernel row of a grant: `(request_id, status, deadline, actions)`.
fn newest(db: &DbHandle, grant_id: &str) -> Option<(String, String, String, Vec<String>)> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT request_id, status, deadline, action_payload FROM approval_requests
         WHERE source_store = 'task_grant' AND source_ref = ?1 ORDER BY created_at DESC LIMIT 1",
        params![grant_id],
        |r| {
            let payload: String = r.get(3)?;
            let actions = serde_json::from_str::<Value>(&payload)
                .ok()
                .and_then(|v| v.get("actions").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
                .unwrap_or_default();
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, actions))
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// Open (and, for a person's `card` grant, approve) the kernel row for a grant's current action
/// set. An open row for an older action set is withdrawn first.
pub fn link(db: &DbHandle, g: &Grant, by_person: bool) -> Result<Option<String>, AuthorityError> {
    if super::mode() == Mode::Off {
        return Ok(None);
    }
    let mut want = g.actions.clone();
    want.sort();
    if let Some((rid, status, _, actions)) = newest(db, &g.id) {
        if actions == want && (status == "approved" || (status == "open" && !by_person)) {
            return Ok(Some(rid));
        }
        let _ = rid;
        revoke(db, &g.id, "the grant's actions changed; a new request covers the full set");
    }
    let Some(rid) = super::open(db, open_for(g))? else { return Ok(None) };
    if by_person {
        let k = super::kernel(db, crate::authority::audit::DEFAULT_TENANT)?;
        let person = Caller { kind: CallerKind::Human, principal: g.owner.clone() };
        let note = Some(format!("pressed \"Allow for this task\" ({})", if g.source_ref.is_empty() { "card" } else { g.source_ref.as_str() }));
        k.resolve(&person, ResolveBody { request_id: rid.clone(), decision: Decision::Approve, note })?;
    }
    Ok(Some(rid))
}

/// [`link`], logging instead of failing: a grant is still recorded when the kernel is down
/// (its live check then follows the mode).
pub fn link_logged(db: &DbHandle, g: &Grant, by_person: bool) {
    if let Err(e) = link(db, g, by_person) {
        tracing::warn!(grant = %g.id, code = ?e.code, error = %e.message, "task grant: kernel approval not opened");
    }
}

/// The live check for one use of a grant: a kernel `check` receipt with the grant as the token
/// source. `true` lets the action go on under the grant.
pub fn admit(db: &DbHandle, g: &Grant, action: &str) -> bool {
    let mode = super::mode();
    if mode == Mode::Off {
        return true;
    }
    let row = newest(db, &g.id);
    let now = Utc::now();
    let ok = row.as_ref().is_some_and(|(_, status, deadline, actions)| {
        status == "approved"
            && actions.iter().any(|a| a == action)
            && DateTime::parse_from_rfc3339(deadline).map(|d| d.with_timezone(&Utc) > now).unwrap_or(false)
    });
    let decision = match (ok, mode) {
        (true, _) => "allow",
        (false, Mode::Enforce) => "deny",
        (false, _) => "shadow_deny",
    };
    let payload = json!({
        "event": "check", "token_source": "task_grant", "grant_id": g.id, "action": action,
        "approval_request_id": row.as_ref().map(|r| r.0.clone()), "approval_status": row.as_ref().map(|r| r.1.clone()),
        "thread_id": g.thread_id, "bot_id": g.bot_id, "mode": mode.as_str(),
    });
    let written = super::append_receipt(db, &format!("thread:{}", g.thread_id), &g.owner, crate::authority::audit::DEFAULT_TENANT, ReceiptKind::Check, CHECK_SCHEMA_ID, decision, payload);
    match (written, mode) {
        // Write before act: no receipt, no allow under enforce.
        (Err(e), Mode::Enforce) => {
            tracing::warn!(grant = %g.id, code = ?e.code, error = %e.message, "task grant check: no receipt, refused (enforce)");
            false
        }
        (Err(e), _) => {
            tracing::debug!(grant = %g.id, code = ?e.code, error = %e.message, "task grant check: no receipt (shadow)");
            true
        }
        (Ok(_), Mode::Enforce) => ok,
        (Ok(_), _) => true,
    }
}

/// The grant ended (revoked by the owner, the thread closed, or denied in the kernel inbox):
/// close its kernel row, approved or still open.
pub fn revoke(db: &DbHandle, grant_id: &str, reason: &str) {
    if super::mode() == Mode::Off {
        return;
    }
    let Some((rid, status, _, _)) = newest(db, grant_id) else { return };
    if status != "open" && status != "approved" {
        return;
    }
    let Some(row) = super::linked(db, &rid) else { return };
    match super::kernel(db, &row.tenant).and_then(|k| k.revoke(&rid, reason, serde_json::Map::new())) {
        Ok(_) => {}
        Err(e) => tracing::warn!(grant = grant_id, code = ?e.code, error = %e.message, "task grant: kernel revoke failed"),
    }
}

/// Kernel inbox resolved a grant's request: approve needs nothing more (the live check reads the
/// kernel row); deny revokes the grant. Never calls the kernel's resolve.
pub fn write_back(db: &DbHandle, grant_id: &str, owner: &str, approve: bool) {
    if !approve {
        crate::task_grants::revoke(db, owner, grant_id);
    }
}
