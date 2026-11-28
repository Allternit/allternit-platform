//! The old approval stores write through the kernel (plan step 11a; CONTRACT.md 6.1).
//!
//! Old tables, routes and responses stay. Each adapter does three things:
//!
//! 1. **Create:** when the old store creates a pending approval, [`open`] also calls
//!    `approval.request` (same action, class, owner, deadline) and the old row keeps the kernel
//!    `request_id`. `source_store` + `source_ref` on `approval_requests` point back at the old row.
//! 2. **Resolve:** an old resolve route first decides who is calling with [`gate`] (a person's
//!    session, or refused), then calls [`resolve`] (`approval.resolve`, human only), then updates
//!    the old row. Resolving in the kernel instead (`POST /api/v1/kernel/approvals/resolve`) runs
//!    [`write_back`], which updates the old row WITHOUT calling the kernel again: one
//!    `approval_requests` row and one approve/deny receipt per approval, whichever surface
//!    approved, and no loop.
//! 3. **Consume:** where the old store spends an approval (the ACI grant redeem), [`redeem`]
//!    spends the kernel token too.
//!
//! `ALLTERNIT_KERNEL_CHECK` decides how hard this is (the same switch as `check`):
//! `off` skips the kernel; `shadow` (default) writes through and logs kernel failures but lets the
//! old route finish; `enforce` refuses the old resolve when the kernel refuses. A credential that
//! is never a person (agent, run, API key, MCP client, internal service, access or worker token)
//! is refused on every old resolve route in every mode.
//!
//! | Store | Adapter | Create | Resolve | Consume |
//! |---|---|---|---|---|
//! | `factory_approvals` | [`factory`] | `sync_from_ledger` | `factory_approvals::resolve` (app, push, inbox, channels) | the Gate event |
//! | `ApprovalStore` (`/beta/approvals`) + ACI grants | [`tool_approvals`] | `ApprovalStore::create_in` | `/beta/approvals/:id/{approve,deny}`, `/aci/handoff/:id/{approve,deny}` | `aci.run` grant redeem |

pub mod factory;
pub mod tool_approvals;

use std::path::PathBuf;
use std::sync::Arc;

use allternit_authority::{
    ActionClass, ApprovalRedeemBody, ApprovalRequestV1, AuthorityError, Caller, CallerKind, ErrorCode, SourceStore,
    OS_EXTENSION_KEY,
};
use axum::http::HeaderMap;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use super::approvals::{Decision, Resolution, ResolveBody, SqlAuthority, REQUEST_SCHEMA_ID};
use super::check::Mode;
use super::tokens;
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

/// The message every old resolve route answers a non-person caller with.
pub const NOT_A_PERSON: &str =
    "only a person's session can approve or deny; agent, run, API-key, MCP and service credentials are refused";

fn data_dir(db: &DbHandle) -> PathBuf {
    db.path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
}

/// The kernel for `tenant`, with the process signer.
pub fn kernel(db: &DbHandle, tenant: &str) -> Result<SqlAuthority, AuthorityError> {
    let signer = tokens::kernel_signer(&data_dir(db))?;
    Ok(SqlAuthority::new(db.clone(), signer).with_tenant(Some(tenant.to_string())))
}

pub fn mode() -> Mode {
    Mode::from_env()
}

/// What an old store asks the kernel to open.
pub struct Open {
    pub store: SourceStore,
    /// The old row's id.
    pub source_ref: String,
    /// The account that owns the approval (its person resolves it).
    pub owner: String,
    /// Chain scope: [`super::kernel_scope`] when the store knows the caller, else `default`.
    pub tenant: String,
    pub run_id: String,
    /// The acting side, never the owner's id: `<store>:<ref>`.
    pub principal: String,
    pub class: ActionClass,
    pub summary: String,
    pub payload: Map<String, Value>,
    pub deadline: DateTime<Utc>,
}

/// `approval.request` for an old store's new row. The tenant rides in
/// `extensions["x-os-v1"].tenant` so a later resolve or redeem writes on the same chain.
pub fn open_with(k: &SqlAuthority, o: Open) -> Result<String, AuthorityError> {
    let summary: String = o.summary.trim().chars().take(280).collect();
    let summary = if summary.is_empty() { format!("Approve {}", o.source_ref) } else { summary };
    let mut ext = Map::new();
    ext.insert(OS_EXTENSION_KEY.into(), json!({ "tenant": o.tenant }));
    let body = ApprovalRequestV1 {
        schema_id: REQUEST_SCHEMA_ID.into(),
        schema_version: tokens::SCHEMA_VERSION.into(),
        request_id: None,
        source_store: o.store,
        source_ref: Some(o.source_ref.clone()),
        run_id: o.run_id,
        principal: o.principal,
        assignee: "owner".into(),
        action_class: o.class,
        action_summary: summary,
        action_payload: o.payload,
        action_hash: None,
        deadline: tokens::ts(o.deadline),
        on_expiry: "reject".into(),
        attention_id: None,
        check_id: format!("{}:{}", store_str(o.store), o.source_ref),
        status: None,
        created_at: None,
        resolved_at: None,
        resolved_by: None,
        evidence_refs: None,
        extensions: Some(ext),
    };
    // The old store's own caller opened it: a run or host code, never a person resolving.
    let caller = Caller { kind: CallerKind::Service, principal: o.owner };
    k.request(&caller, body)?.request_id.ok_or_else(|| AuthorityError::kernel_unavailable("kernel returned no request id"))
}

/// [`open_with`] on the process kernel. `Ok(None)` when the kernel is off. A deadline already
/// past (or under a minute away) is moved to one minute from now, so a just-expiring old row
/// still gets its kernel row.
pub fn open(db: &DbHandle, mut o: Open) -> Result<Option<String>, AuthorityError> {
    if mode() == Mode::Off {
        return Ok(None);
    }
    let floor = Utc::now() + Duration::minutes(1);
    if o.deadline < floor {
        o.deadline = floor;
    }
    let k = kernel(db, &o.tenant.clone())?;
    open_with(&k, o).map(Some)
}

pub fn store_str(s: SourceStore) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// The kernel row behind an old row, newest first.
pub fn request_for(db: &DbHandle, store: SourceStore, source_ref: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT request_id FROM approval_requests WHERE source_store = ?1 AND source_ref = ?2 ORDER BY created_at DESC LIMIT 1",
        params![store_str(store), source_ref],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// A kernel row as the adapters need it.
pub struct Linked {
    pub store: Option<SourceStore>,
    pub source_ref: Option<String>,
    pub owner: String,
    pub status: String,
    pub tenant: String,
    pub action_hash: String,
}

pub fn linked(db: &DbHandle, request_id: &str) -> Option<Linked> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT source_store, source_ref, owner_user_id, status, extensions, action_hash FROM approval_requests WHERE request_id = ?1",
        params![request_id],
        |r| {
            let store: String = r.get(0)?;
            let ext: Option<String> = r.get(4)?;
            let tenant = ext
                .and_then(|e| serde_json::from_str::<Value>(&e).ok())
                .and_then(|v| v.get(OS_EXTENSION_KEY)?.get("tenant")?.as_str().map(str::to_string))
                .unwrap_or_else(|| super::audit::DEFAULT_TENANT.to_string());
            Ok(Linked {
                store: serde_json::from_value(Value::String(store)).ok(),
                source_ref: r.get(1)?,
                owner: r.get(2)?,
                status: r.get(3)?,
                tenant,
                action_hash: r.get(5)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// Who is calling an old resolve route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// A person's session (Clerk session for this user, Desktop window proof, local-dev bypass).
    Person,
    /// No machine credential, but no person proof either (an old client that sends no
    /// `x-allternit-human-proof`). Shadow and off let the old route finish and withdraw the kernel
    /// row; enforce refuses.
    Unproven(&'static str),
    /// Refuse: a machine credential in any mode, or an unproven caller under enforce.
    Refuse(&'static str),
}

/// Decide who is calling an old resolve route (same rules as the kernel's own resolve route,
/// `authority::routes`).
pub async fn gate(state: &AppState, headers: &HeaderMap, user: &AuthUser, api_credential: bool) -> Gate {
    gate_in(state, headers, user, api_credential, mode()).await
}

pub async fn gate_in(state: &AppState, headers: &HeaderMap, user: &AuthUser, api_credential: bool, mode: Mode) -> Gate {
    if let Some((_, reason)) = super::routes::machine_marker(headers, api_credential, &user.user_id) {
        tracing::info!(reason, user_id = %user.user_id, "old approval route: machine caller refused");
        return Gate::Refuse(NOT_A_PERSON);
    }
    match crate::subscription_routes::person_proof(state, headers, &user.user_id).await {
        Ok(_) => Gate::Person,
        Err(reason) if mode == Mode::Enforce => {
            tracing::info!(reason, user_id = %user.user_id, "old approval route: no person proof, refused (enforce)");
            Gate::Refuse("approve or deny from a signed-in Allternit app: this request carried no person's session")
        }
        Err(reason) => Gate::Unproven(reason),
    }
}

/// Outcome of the kernel side of an old-route resolve.
#[derive(Debug)]
pub enum Kernel {
    /// The kernel resolved it: one approve/deny receipt. `token` on approve.
    Resolved { request_id: String, token: Option<String> },
    /// Already resolved in the kernel the same way (the kernel inbox went first and its
    /// write-back did not reach the old row): finish the old row, no new receipt.
    AlreadySame { request_id: String },
    /// Kernel off, or shadow and the kernel could not act: the old route goes on.
    Skipped,
}

/// The kernel side of an old-route resolve, run after [`gate`].
///
/// `owner` is the old row's owner; the old route has already checked the caller may decide for
/// it, so the kernel caller is that owner as a person. `make` opens a kernel row for an old row
/// that has none (created before this adapter, or the kernel was down at create time).
pub fn resolve(
    db: &DbHandle,
    gate: &Gate,
    request_id: Option<String>,
    owner: &str,
    approve: bool,
    note: Option<String>,
    make: impl FnOnce() -> Result<Option<String>, AuthorityError>,
) -> Result<Kernel, AuthorityError> {
    if let Gate::Refuse(why) = gate {
        return Err(AuthorityError::without_receipt(ErrorCode::ForbiddenCaller, *why));
    }
    let mode = mode();
    if mode == Mode::Off {
        return Ok(Kernel::Skipped);
    }
    let soft = |e: AuthorityError| -> Result<Kernel, AuthorityError> {
        if mode == Mode::Enforce {
            Err(e)
        } else {
            tracing::warn!(code = ?e.code, error = %e.message, "kernel write-through failed (shadow): old route continues");
            Ok(Kernel::Skipped)
        }
    };
    let request_id = match request_id.filter(|r| !r.is_empty()) {
        Some(r) => r,
        None => match make() {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(Kernel::Skipped),
            Err(e) => return soft(e),
        },
    };
    let Some(row) = linked(db, &request_id) else {
        return soft(AuthorityError::without_receipt(ErrorCode::NotFound, "kernel request for this approval is missing"));
    };
    let k = match kernel(db, &row.tenant) {
        Ok(k) => k,
        Err(e) => return soft(e),
    };
    match gate {
        Gate::Refuse(_) => unreachable!("refused above"),
        Gate::Unproven(why) => {
            // Shadow: the old route finishes; the kernel row is closed so no surface shows it open.
            let _ = k.withdraw(&request_id, &format!("resolved through an old route without a person proof ({why})"));
            return Ok(Kernel::Skipped);
        }
        Gate::Person => {}
    }
    let same = if approve { "approved" } else { "denied" };
    if row.status == same {
        return Ok(Kernel::AlreadySame { request_id });
    }
    if row.owner != owner {
        return soft(AuthorityError::without_receipt(ErrorCode::NotFound, "kernel request belongs to another account"));
    }
    let caller = Caller { kind: CallerKind::Human, principal: row.owner.clone() };
    let body = ResolveBody { request_id: request_id.clone(), decision: if approve { Decision::Approve } else { Decision::Deny }, note };
    match k.resolve(&caller, body) {
        Ok(Resolution::Approved(t)) => Ok(Kernel::Resolved { request_id, token: Some(t.token) }),
        Ok(Resolution::Denied(_)) => Ok(Kernel::Resolved { request_id, token: None }),
        // A conflict (resolved the other way, expired) is a real refusal in every mode except off:
        // the old row must not say approved while the kernel says denied.
        Err(e) if e.code == ErrorCode::Conflict => Err(e),
        Err(e) => soft(e),
    }
}

/// `approval.redeem` of the kernel token an old store kept, for the payload it was opened with.
/// `Ok(None)` when the kernel is off.
pub fn redeem(db: &DbHandle, request_id: &str, owner: &str, token: &str) -> Result<Option<String>, AuthorityError> {
    if mode() == Mode::Off {
        return Ok(None);
    }
    let row = linked(db, request_id)
        .ok_or_else(|| AuthorityError::without_receipt(ErrorCode::TokenInvalid, "kernel request for this approval is missing"))?;
    let k = kernel(db, &row.tenant)?;
    let caller = Caller { kind: CallerKind::Run, principal: owner.to_string() };
    k.redeem(&caller, ApprovalRedeemBody { token: token.to_string(), action_hash: row.action_hash })
        .map(|r| Some(r.spend_receipt_id))
}

/// Close the kernel row of an old row that closed on its own (ledger, expiry).
pub fn withdraw(db: &DbHandle, request_id: Option<&str>, reason: &str) {
    if mode() == Mode::Off {
        return;
    }
    let Some(rid) = request_id.filter(|r| !r.is_empty()) else { return };
    let Some(row) = linked(db, rid) else { return };
    if row.status != "open" {
        return;
    }
    match kernel(db, &row.tenant).and_then(|k| k.withdraw(rid, reason)) {
        Ok(_) => {}
        Err(e) => tracing::warn!(code = ?e.code, error = %e.message, request_id = rid, "kernel withdraw failed"),
    }
}

/// Kernel inbox → old row. Called by `POST /api/v1/kernel/approvals/resolve` after the kernel
/// resolved `request_id`. Updates the old store directly (never through its kernel-calling
/// resolve), so there is no loop and no second receipt. `token` is the approve token.
pub async fn write_back(state: &Arc<AppState>, request_id: &str, approve: bool, token: Option<String>, actor: &str) {
    let Some(row) = linked(&state.db, request_id) else { return };
    let Some(source_ref) = row.source_ref.clone() else { return };
    match row.store {
        Some(SourceStore::Factory) => factory::write_back(state, &source_ref, approve, actor).await,
        Some(SourceStore::PermissionPolicy) | Some(SourceStore::Aci) => {
            tool_approvals::write_back(state, &source_ref, request_id, approve, token)
        }
        // Kernel-native rows (from `check`) have no old row; gateway, cowork and task grants are
        // wired in a later run of step 11.
        _ => {}
    }
}
