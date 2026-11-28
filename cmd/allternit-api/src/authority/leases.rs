//! `lease` and `GET /api/v1/kernel/leases` (CONTRACT.md 2.2, 3; plan step 12).
//!
//! `computer_control_lease.rs` stays the store: one controller per computer, 120 s TTL, renewed by a
//! heartbeat, a person preempts an agent. This module is the ABI view over it. A control lease is
//! shown as a `LeaseV1` (`lease_id` is `ccl_<computer_id>`, `scope` is `["computer:<id>"]`,
//! `state_version` is always 0 because the store keeps no version) and the crate's
//! `lease.acquire` / `lease.heartbeat` / `lease.release` act on it with a `lease` receipt written
//! before the change. Control leases are the only lease kind in allternit-api today; an unknown
//! scope kind is `bad_request`, not a silent no-op.
//!
//! Acquire needs `check_id` to name an ALLOW `check` receipt of the same account, else `denied`.
//! Who may hold: a person's session holds as `user`; a run holds as `agent`, `bot` or `session`,
//! never as `user`.

use std::sync::Arc;

use allternit_authority::{
    AuthorityError, Caller, CallerKind, ErrorCode, LeaseAcquireBody, LeaseReleaseResult, LeaseRequest, LeaseResponse, ReceiptKind,
};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Map, Value};

use super::approvals::SqlAuthority;
use super::audit::{append, append_in_tx, ReceiptInput};
use super::tokens;
use crate::auth::AuthUser;
use crate::computer_control_lease::{self as ccl, Holder, HolderKind, Lease, LEASE_TTL_SECS};
use crate::enterprise_auth::CredentialContext;
use crate::AppState;

pub const LEASE_SCHEMA_ID: &str = "allternit.kernel.LeaseV1";
const LEASE_ID_PREFIX: &str = "ccl_";
const COMPUTER_SCOPE: &str = "computer:";

fn bad(msg: impl Into<String>) -> AuthorityError {
    AuthorityError::without_receipt(ErrorCode::BadRequest, msg)
}

fn store(e: impl std::fmt::Display) -> AuthorityError {
    AuthorityError::kernel_unavailable(format!("lease store: {e}"))
}

/// The `LeaseV1` of one control lease.
pub fn lease_v1(l: &Lease) -> Map<String, Value> {
    let expires = tokens::parse_ts(&l.expires_at);
    let heartbeat = expires.map(|e| tokens::ts(e - chrono::Duration::seconds(LEASE_TTL_SECS))).unwrap_or_else(|| l.acquired_at.clone());
    let v = json!({
        "schema_id": LEASE_SCHEMA_ID,
        "schema_version": "1.0.0",
        "lease_id": format!("{LEASE_ID_PREFIX}{}", l.computer_id),
        "holder": format!("{}:{}", l.holder.kind.as_str(), l.holder.id),
        "host": l.computer_id,
        "process_or_session": l.holder.device_id.clone().unwrap_or_else(|| l.holder.id.clone()),
        "scope": [format!("{COMPUTER_SCOPE}{}", l.computer_id)],
        "acquired_at": l.acquired_at,
        "heartbeat_at": heartbeat,
        "heartbeat_interval_ms": (LEASE_TTL_SECS * 1000 / 3),
        "expires_after_ms": (LEASE_TTL_SECS * 1000),
        "reclaim_policy": "NEEDS_HUMAN",
        "state_version": 0,
        "extensions": { "x-os-v1": { "kind": "computer_control", "expires_at": l.expires_at, "holder_label": l.holder.label } },
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Live control leases the account may see: on computers it owns (directly or through a bot) and
/// ones it holds itself. Read-only; expired leases are not listed.
pub fn list_live(conn: &Connection, owner: &str, now: i64) -> Result<Vec<Map<String, Value>>, AuthorityError> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT l.computer_id FROM computer_control_leases l
             LEFT JOIN computers c ON c.id = l.computer_id
             WHERE (c.owner_type = 'user' AND c.owner_id = ?1)
                OR c.bot_id IN (SELECT id FROM agents WHERE user_id = ?1)
                OR (l.holder_kind = 'user' AND l.holder_id = ?1)
             ORDER BY l.computer_id",
        )
        .map_err(store)?;
    let ids: Vec<String> = stmt.query_map(params![owner], |r| r.get(0)).map_err(store)?.filter_map(Result::ok).collect();
    let mut out = Vec::new();
    for id in ids {
        if let Some(l) = ccl::current(conn, &id, now).map_err(store)? {
            out.push(lease_v1(&l));
        }
    }
    Ok(out)
}

fn owns_computer(conn: &Connection, owner: &str, computer_id: &str, now: i64) -> Result<bool, AuthorityError> {
    Ok(list_live(conn, owner, now)?.iter().any(|l| l.get("host").and_then(Value::as_str) == Some(computer_id)))
}

/// The holder the caller may take: a person holds as `user`, a run as `agent` / `bot` / `session`.
fn holder_for(caller: &Caller, body: &LeaseAcquireBody) -> Result<Holder, AuthorityError> {
    let (kind, id) = match body.holder.split_once(':') {
        Some((k, id)) => (k, id),
        None => ("agent", body.holder.as_str()),
    };
    let kind = match kind {
        "user" => HolderKind::User,
        "agent" => HolderKind::Agent,
        "bot" => HolderKind::Bot,
        "session" => HolderKind::Session,
        other => return Err(bad(format!("unknown holder kind {other}"))),
    };
    match (caller.kind, kind) {
        (CallerKind::Human, HolderKind::User) if id == caller.principal => {
            Ok(Holder { kind, id: id.into(), label: None, device_id: Some(body.process_or_session.clone()).filter(|d| !d.is_empty()) })
        }
        (CallerKind::Human, _) => Err(AuthorityError::without_receipt(ErrorCode::ForbiddenCaller, "a person holds a lease as themselves (user:<own id>)")),
        (_, HolderKind::User) => Err(AuthorityError::without_receipt(ErrorCode::ForbiddenCaller, "a run cannot hold a lease as a user")),
        _ => Ok(Holder { kind, id: id.into(), label: None, device_id: None }),
    }
}

fn computer_of(scope: &[String]) -> Result<&str, AuthorityError> {
    match scope {
        [one] => one
            .strip_prefix(COMPUTER_SCOPE)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| bad("only computer:<id> leases are wired in allternit-api (control leases)")),
        _ => Err(bad("scope must name exactly one computer:<id>")),
    }
}

fn computer_of_lease_id(id: &str) -> Result<&str, AuthorityError> {
    id.strip_prefix(LEASE_ID_PREFIX).filter(|c| !c.is_empty()).ok_or_else(|| AuthorityError::without_receipt(ErrorCode::NotFound, "no such lease"))
}

fn allow_check_exists(conn: &Connection, owner: &str, check_id: &str) -> Result<bool, AuthorityError> {
    conn.query_row(
        "SELECT 1 FROM kernel_receipts WHERE receipt_id = ?1 AND kind = 'check' AND decision = 'allow' AND principal = ?2",
        params![check_id, owner],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
    .map_err(store)
}

fn rin<'a>(owner: &'a str, tenant: &'a str, run: &'a str, decision: &'a str, payload: Value) -> ReceiptInput<'a> {
    ReceiptInput { run_id: run, principal: owner, tenant, kind: ReceiptKind::Lease, schema_id: LEASE_SCHEMA_ID, decision, payload, idempotency_key: None }
}

/// `lease.acquire`, `lease.heartbeat`, `lease.release` over the control-lease store.
pub fn lease(k: &SqlAuthority, caller: &Caller, req: LeaseRequest) -> Result<LeaseResponse, AuthorityError> {
    let (db, signer, tenant, now_dt) = k.parts();
    let now = now_dt.timestamp();
    let owner = caller.principal.as_str();
    let receipt = |run: &'static str, decision: &'static str, payload: Value| rin(owner, &tenant, run, decision, payload);
    let deny = |code: ErrorCode, msg: &str, payload: Value| -> AuthorityError {
        let written = db.connect().map_err(store).and_then(|mut c| append(&mut c, &signer, receipt("lease", "denied", payload), now_dt));
        match written {
            Ok(rid) => AuthorityError::new(code, msg, Some(rid)),
            Err(e) => e,
        }
    };
    let mut conn = db.connect().map_err(store)?;
    match req {
        LeaseRequest::Acquire(body) => {
            let computer = computer_of(&body.scope)?.to_string();
            let holder = holder_for(caller, &body)?;
            if !allow_check_exists(&conn, owner, &body.check_id)? {
                return Err(deny(ErrorCode::Denied, "no ALLOW check for this lease", json!({ "event": "acquire", "computer_id": computer, "check_id": body.check_id })));
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(store)?;
            append_in_tx(
                &tx,
                &signer,
                receipt("lease", "acquire", json!({ "event": "acquire", "computer_id": computer, "holder": body.holder, "check_id": body.check_id })),
                now_dt,
            )?;
            match ccl::take(&tx, &computer, &holder, now) {
                Ok(l) => {
                    tx.commit().map_err(store)?;
                    Ok(LeaseResponse::Lease(lease_v1(&l)))
                }
                Err(ccl::TakeError::Held(l)) => {
                    drop(tx);
                    Err(deny(ErrorCode::Conflict, "the computer is already leased", json!({ "event": "acquire", "computer_id": computer, "held_by": format!("{}:{}", l.holder.kind.as_str(), l.holder.id) })))
                }
                Err(e) => Err(store(format!("{e:?}"))),
            }
        }
        LeaseRequest::Heartbeat(body) => {
            let computer = computer_of_lease_id(&body.lease_id)?.to_string();
            let live = ccl::current(&conn, &computer, now).map_err(store)?.filter(|_| owns_computer(&conn, owner, &computer, now).unwrap_or(false));
            let Some(live) = live else {
                return Err(AuthorityError::without_receipt(ErrorCode::NotFound, "no live lease with that id"));
            };
            if body.state_version != 0 {
                return Err(deny(ErrorCode::Conflict, "stale state_version", json!({ "event": "heartbeat", "lease_id": body.lease_id, "state_version": body.state_version })));
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(store)?;
            append_in_tx(&tx, &signer, receipt("lease", "heartbeat", json!({ "event": "heartbeat", "lease_id": body.lease_id })), now_dt)?;
            let renewed = ccl::take(&tx, &computer, &live.holder, now).map_err(|e| store(format!("{e:?}")))?;
            tx.commit().map_err(store)?;
            Ok(LeaseResponse::Lease(lease_v1(&renewed)))
        }
        LeaseRequest::Release(body) => {
            let computer = computer_of_lease_id(&body.lease_id)?.to_string();
            let live = ccl::current(&conn, &computer, now).map_err(store)?.filter(|_| owns_computer(&conn, owner, &computer, now).unwrap_or(false));
            let Some(live) = live else {
                return Err(AuthorityError::without_receipt(ErrorCode::NotFound, "no live lease with that id"));
            };
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(store)?;
            let rid = append_in_tx(&tx, &signer, receipt("lease", "release", json!({ "event": "release", "lease_id": body.lease_id })), now_dt)?;
            ccl::release(&tx, &computer, &live.holder, now).map_err(store)?;
            tx.commit().map_err(store)?;
            Ok(LeaseResponse::Released(LeaseReleaseResult { released: true, receipt_id: rid }))
        }
    }
}

/// `GET /api/v1/kernel/leases`: live leases for the caller's scope. A person's session or a run
/// of the same account may read; service, API-key and MCP callers may not. Read-only, no receipt.
pub async fn list_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
) -> Response {
    let caller = super::routes::caller_for(&state, &headers, &user, credential.is_some()).await;
    if !matches!(caller.kind, CallerKind::Human | CallerKind::Run) {
        let e = AuthorityError::without_receipt(ErrorCode::ForbiddenCaller, "only a person's session or a run can list leases");
        return (StatusCode::FORBIDDEN, Json(json!({ "ok": false, "error": e }))).into_response();
    }
    let (db, owner) = (state.db.clone(), caller.principal);
    let out = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(store)?;
        list_live(&conn, &owner, Utc::now().timestamp())
    })
    .await;
    match out {
        Ok(Ok(leases)) => (StatusCode::OK, Json(json!({ "ok": true, "body": { "leases": leases } }))).into_response(),
        Ok(Err(e)) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "ok": false, "error": e }))).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "ok": false, "error": store(e) }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use chrono::DateTime;
    use allternit_factory_engine::receipts::sign::ReceiptSigner;

    const USER: &str = "user_l";

    fn t0() -> DateTime<Utc> {
        tokens::parse_ts("2026-11-28T12:00:00Z").unwrap()
    }
    fn kernel(db: &DbHandle) -> SqlAuthority {
        SqlAuthority::new(db.clone(), Arc::new(ReceiptSigner::from_seed([5u8; 32]))).at(t0())
    }
    fn human() -> Caller {
        Caller { kind: CallerKind::Human, principal: USER.into() }
    }
    fn allow_check(db: &DbHandle) -> String {
        let mut conn = db.connect().unwrap();
        append(
            &mut conn,
            &ReceiptSigner::from_seed([5u8; 32]),
            ReceiptInput {
                run_id: "run_l",
                principal: USER,
                tenant: "default",
                kind: ReceiptKind::Check,
                schema_id: "allternit.os.PolicyCheckV1",
                decision: "allow",
                payload: json!({ "event": "check" }),
                idempotency_key: None,
            },
            t0(),
        )
        .unwrap()
    }

    #[test]
    fn leases_route_lists_a_live_control_lease_and_hides_expired_and_foreign_ones() {
        let db = DbHandle::new_memory().unwrap();
        let k = kernel(&db);
        let check_id = allow_check(&db);
        let body = LeaseAcquireBody {
            holder: format!("user:{USER}"),
            scope: vec!["computer:mac_1".into()],
            host: "mac_1".into(),
            process_or_session: "dev_1".into(),
            heartbeat_interval_ms: 40_000,
            expires_after_ms: 120_000,
            reclaim_policy: allternit_authority::ReclaimPolicy::NeedsHuman,
            check_id,
        };
        let got = lease(&k, &human(), LeaseRequest::Acquire(body.clone())).unwrap();
        let LeaseResponse::Lease(v) = got else { panic!("expected a lease") };
        assert_eq!(v["lease_id"], "ccl_mac_1");

        let conn = db.connect().unwrap();
        let live = list_live(&conn, USER, t0().timestamp()).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0]["scope"][0], "computer:mac_1");
        assert_eq!(live[0]["schema_id"], LEASE_SCHEMA_ID);
        // Expired after the TTL, and another account sees none.
        assert!(list_live(&conn, USER, t0().timestamp() + LEASE_TTL_SECS + 1).unwrap().is_empty());
        assert!(list_live(&conn, "someone_else", t0().timestamp()).unwrap().is_empty());

        // No ALLOW check, no lease; a run cannot hold as a user.
        let mut no_check = body.clone();
        no_check.check_id = "chk_missing".into();
        assert_eq!(lease(&k, &human(), LeaseRequest::Acquire(no_check)).unwrap_err().code, ErrorCode::Denied);
        let run = Caller { kind: CallerKind::Run, principal: USER.into() };
        assert_eq!(lease(&k, &run, LeaseRequest::Acquire(body)).unwrap_err().code, ErrorCode::ForbiddenCaller);
    }
}
