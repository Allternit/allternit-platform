//! `permission_policy::ApprovalStore` (`/beta/approvals`) and the ACI `ActionGrantStore` through
//! the kernel. They are one approval: an ACI handoff creates the `ApprovalStore` record and issues
//! the grant under the same id, so they share ONE kernel row. `source_store` is `aci` for ACI and
//! computer handoffs and `permission_policy` for policy asks; `source_ref` is the record id. The
//! in-memory record keeps `kernel_request_id` and, once approved, `kernel_token`.
//!
//! - Create: `ApprovalStore::create_in` (every create site: `tool_routes` ask path, `aci.run`
//!   handoff, `aci_safety` computer confirmations, beta session tool asks) calls [`link`].
//! - Resolve: `/beta/approvals/:id/{approve,deny}` and `/aci/handoff/:id/{approve,deny}` call
//!   [`resolve`] after the person gate.
//! - Kernel inbox: [`write_back`] marks the record and the grant, no kernel call.
//! - Consume: the `aci.run` grant redeem calls [`redeem`], which spends the kernel token.
//!
//! The ACI grant's own model (bound action hash, single use, owner, expiry) is the model
//! `ApprovalTokenV1` took over (CONTRACT.md 6.1 row 2); the in-memory grant stays as the
//! old-client view and the kernel token is the authority.

use std::sync::Arc;

use allternit_authority::{ActionClass, AuthorityError, ErrorCode, SourceStore};
use chrono::{Duration, Utc};
use serde_json::json;

use super::{Gate, Kernel, Open};
use crate::authority::check::{self, Action, CheckCtx, Mode, Source};
use crate::db::DbHandle;
use crate::permission_policy::{ApprovalRequest, ApprovalStatus};
use crate::AppState;

const DEADLINE_MINS: i64 = 60;

fn is_aci(tool: &str) -> bool {
    tool.starts_with("aci.") || tool.starts_with("computer.")
}

/// The kernel request for a record: the exact tool and arguments a person approves.
pub fn open_for(db: &DbHandle, req: &ApprovalRequest, tenant: String) -> Open {
    let aci = is_aci(&req.tool);
    let mut payload = serde_json::Map::new();
    payload.insert("store".into(), json!(if aci { "aci" } else { "permission_policy" }));
    payload.insert("approval_id".into(), json!(req.id));
    payload.insert("tool".into(), json!(req.tool));
    payload.insert("args".into(), req.args.clone());
    // ACI handoffs gate sensitive browser/computer actions (pay, submit, credential fill):
    // strictest class. Policy asks use the kernel's own tool table.
    let class = if aci {
        ActionClass::Irreversible
    } else {
        check::classify(&CheckCtx::new(db.clone(), &req.user_id, None), &Action::new(Source::Native, &req.tool, &req.args)).class
    };
    let deadline = if aci {
        Utc::now() + Duration::seconds(crate::aci_approvals::grant_ttl_secs() as i64)
    } else {
        Utc::now() + Duration::minutes(DEADLINE_MINS)
    };
    Open {
        store: if aci { SourceStore::Aci } else { SourceStore::PermissionPolicy },
        source_ref: req.id.clone(),
        owner: req.user_id.clone(),
        tenant,
        run_id: req.args.get("run_id").and_then(|v| v.as_str()).filter(|r| !r.is_empty()).map_or_else(|| format!("approval:{}", req.id), str::to_string),
        principal: format!("{}:{}", if aci { "aci" } else { "permission_policy" }, req.id),
        class,
        summary: format!("Run {}", req.tool),
        payload,
        deadline,
    }
}

/// Open the kernel row for a new record. `None` when the kernel is off or could not open it
/// (logged; the first resolve opens it lazily).
pub fn link(db: &DbHandle, req: &ApprovalRequest, tenant: String) -> Option<String> {
    match super::open(db, open_for(db, req, tenant)) {
        Ok(rid) => rid,
        Err(e) => {
            tracing::warn!(approval = %req.id, code = ?e.code, error = %e.message, "approval store: kernel request not opened");
            None
        }
    }
}

/// The kernel side of `/beta/approvals/:id/{approve,deny}` and `/aci/handoff/:id/{approve,deny}`.
/// Run after the old route's own owner check and [`super::gate`]. `Err` means refuse the old
/// resolve (the kernel refused, or a machine caller).
///
/// The kernel caller is the record's owner as a person; `decided_by` (the signed-in account,
/// which may be the computer's owner rather than the run's) goes in the resolve note.
pub async fn resolve(state: &Arc<AppState>, gate: &Gate, id: &str, approve: bool, decided_by: &str) -> Result<(), AuthorityError> {
    let Some(req) = state.approval_store.get(id) else {
        return Err(AuthorityError::without_receipt(ErrorCode::NotFound, "approval request not found"));
    };
    let Some(db) = state.approval_store.kernel_db().cloned() else {
        // Not linked (tests and tools that build their own store): the person gate still applies.
        return match gate {
            Gate::Refuse(why) => Err(AuthorityError::without_receipt(ErrorCode::ForbiddenCaller, *why)),
            _ => Ok(()),
        };
    };
    let gate = gate.clone();
    let lazy = req.clone();
    let note = Some(format!("decided by {decided_by}"));
    let out = tokio::task::spawn_blocking(move || {
        let make_db = db.clone();
        super::resolve(&db, &gate, req.kernel_request_id.clone(), &req.user_id, approve, note, move || {
            Ok(link(&make_db, &lazy, crate::authority::audit::DEFAULT_TENANT.to_string()))
        })
    })
    .await
    .map_err(|e| AuthorityError::kernel_unavailable(format!("kernel task: {e}")))??;
    match out {
        Kernel::Resolved { request_id, token } => state.approval_store.set_kernel(id, Some(request_id), token),
        Kernel::AlreadySame { request_id } => state.approval_store.set_kernel(id, Some(request_id), None),
        Kernel::Skipped => {}
    }
    Ok(())
}

/// Kernel inbox resolved a record: mark it and its ACI grant like the old routes do, keep the
/// token. Never calls the kernel.
pub fn write_back(state: &Arc<AppState>, id: &str, request_id: &str, approve: bool, token: Option<String>) {
    let Some(req) = state.approval_store.get(id) else { return };
    if req.status != ApprovalStatus::Pending {
        return;
    }
    if approve {
        state.approval_store.approve(id);
        crate::aci_approvals::GRANTS.approve(id);
    } else {
        state.approval_store.deny(id);
        crate::aci_approvals::GRANTS.deny(id);
    }
    state.approval_store.set_kernel(id, Some(request_id.to_string()), token);
}

/// The `aci.run` grant redeem: also spend the kernel token for this record. Called after the
/// in-memory grant redeemed. `Err(reason)` refuses the run (enforce, or a kernel refusal with a
/// token present); shadow logs a missing token and lets it run.
pub fn redeem(state: &AppState, id: &str) -> Result<(), String> {
    let mode = super::mode();
    if mode == Mode::Off {
        return Ok(());
    }
    let Some(db) = state.approval_store.kernel_db() else { return Ok(()) };
    let req = state.approval_store.get(id);
    let (Some(rid), Some(token)) = (req.as_ref().and_then(|r| r.kernel_request_id.clone()), req.as_ref().and_then(|r| r.kernel_token.clone())) else {
        if mode == Mode::Enforce {
            return Err("this approval has no kernel approval token; approve it again in the Allternit app".into());
        }
        tracing::warn!(approval = id, "aci redeem: no kernel token (shadow), run continues");
        return Ok(());
    };
    let owner = req.map(|r| r.user_id).unwrap_or_default();
    match super::redeem(db, &rid, &owner, &token) {
        Ok(_) => Ok(()),
        Err(e) if mode == Mode::Enforce => Err(e.message),
        Err(e) => {
            tracing::warn!(approval = id, code = ?e.code, error = %e.message, "aci redeem: kernel refused (shadow), run continues");
            Ok(())
        }
    }
}
