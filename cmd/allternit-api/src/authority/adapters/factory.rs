//! `factory_approvals` (V238) through the kernel. `source_store` = `factory`, `source_ref` = the
//! `factory_approvals.id`; the old row keeps the kernel id in `kernel_request_id` (V300).
//!
//! - Create: `factory_approvals::sync_from_ledger` calls [`link`] for each new row.
//! - Resolve: `factory_approvals::resolve` (app route, push action, unified inbox, channel reply)
//!   calls `adapters::resolve` before the Gate event.
//! - Kernel inbox: [`write_back`] runs the Gate event and closes the old row, no kernel call.
//! - Closed by the ledger (answered in Gizzi or the CLI) or expired: [`unlink`] withdraws it.

use std::sync::Arc;

use allternit_authority::{ActionClass, AuthorityError, SourceStore};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde_json::json;

use super::Open;
use crate::db::DbHandle;
use crate::factory_approvals::Approval;
use crate::AppState;

/// The exact action a person approves: this gate on this node of this DAG.
pub fn open_for(a: &Approval) -> Open {
    let mut payload = serde_json::Map::new();
    payload.insert("store".into(), json!("factory"));
    payload.insert("approval_id".into(), json!(a.id));
    payload.insert("dag_id".into(), json!(a.dag_id));
    payload.insert("node_id".into(), json!(a.node_id));
    payload.insert("gate".into(), json!(a.gate));
    Open {
        store: SourceStore::Factory,
        source_ref: a.id.clone(),
        owner: a.owner.clone(),
        // The Factory runs for the owner account; it has no organization at sync time.
        tenant: crate::authority::audit::DEFAULT_TENANT.to_string(),
        run_id: format!("factory:{}", a.dag_id),
        principal: format!("factory:{}/{}", a.dag_id, a.node_id),
        // A high-risk node (money, deploy, client-facing labels) is held to the strictest class.
        class: if a.is_high_risk() { ActionClass::Irreversible } else { ActionClass::RunControl },
        summary: format!("Factory: {}", a.title),
        payload,
        deadline: crate::factory_approvals::parse_deadline(&a.expires_at).unwrap_or_else(|| Utc::now() + crate::factory_approvals::approval_ttl()),
    }
}

/// The kernel id stored on the old row.
pub fn request_id(db: &DbHandle, id: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    conn.query_row("SELECT kernel_request_id FROM factory_approvals WHERE id = ?1", params![id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
}

/// Open the kernel row for a new (or not yet linked) factory approval and store its id.
pub fn link(db: &DbHandle, a: &Approval) -> Result<Option<String>, AuthorityError> {
    let Some(rid) = super::open(db, open_for(a))? else { return Ok(None) };
    if let Ok(conn) = db.connect() {
        let _ = conn.execute("UPDATE factory_approvals SET kernel_request_id = ?2 WHERE id = ?1", params![a.id, rid]);
    }
    Ok(Some(rid))
}

/// [`link`], logging instead of failing: create never blocks on the kernel (its resolve does).
pub fn link_logged(db: &DbHandle, a: &Approval) {
    if let Err(e) = link(db, a) {
        tracing::warn!(approval = %a.id, code = ?e.code, error = %e.message, "factory approval: kernel request not opened");
    }
}

/// The old row closed without a person (ledger, expiry): withdraw its kernel row.
pub fn unlink(db: &DbHandle, id: &str, reason: &str) {
    super::withdraw(db, request_id(db, id).as_deref(), reason);
}

/// Kernel inbox resolved a factory approval: record the Gate event and close the old row.
pub async fn write_back(state: &Arc<AppState>, id: &str, approve: bool, actor: &str) {
    if let Err(e) = crate::factory_approvals::resolve_from_kernel(state, id, approve, actor).await {
        tracing::warn!(approval = id, error = ?e, "factory approval: kernel decision not written back");
    }
}
