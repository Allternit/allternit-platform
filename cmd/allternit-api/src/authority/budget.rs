//! `budget.reserve`, `budget.commit`, `budget.release` (CONTRACT.md 2.6, 3.4; plan step 12).
//!
//! Reserve before spend, commit after, so a run cannot overspend by racing. The limits are the
//! existing ones: a bot's monthly limit lives in `agents.config.spendLimit.monthlyUsd`
//! (`spend_limits.rs`); this module adds no second limits store. Only the reserve -> commit ledger
//! is new (`kernel_budget_reservations`, V310), and it counts against that limit.
//!
//! - Reserve is refused with `budget_exceeded` (and a receipt) when committed + live reservations
//!   this month plus the new amount pass the limit. A principal with no configured limit, or a
//!   non-USD currency, has no limit to pass: the reservation is still recorded and receipted.
//! - Commit with less than the reservation releases the rest; with more, the extra must still fit
//!   the limit or the commit is refused (`budget_exceeded`) and the reservation stays open.
//! - Every reserve, commit, release and refusal is a `budget` receipt written before the change.
//! - Money movement (invoices, charges) is class `money`: `check` already needs a human token for it.
//!   A reservation is not a charge, so it does not.
//!
//! Amounts are decimal strings parsed to millionths; floats never carry money.

use allternit_authority::{
    AuthorityError, BudgetCommitBody, BudgetReleaseBody, BudgetReservationV1, BudgetScope, BudgetStatus, BudgetUsage,
    Caller, ErrorCode, ReceiptKind,
};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

use super::approvals::SqlAuthority;
use super::audit::{append, append_in_tx, ReceiptInput};
use super::tokens;

pub const BUDGET_SCHEMA_ID: &str = "allternit.os.BudgetReservationV1";
const MICROS: i64 = 1_000_000;

fn bad(msg: impl Into<String>) -> AuthorityError {
    AuthorityError::without_receipt(ErrorCode::BadRequest, msg)
}

fn store(e: impl std::fmt::Display) -> AuthorityError {
    AuthorityError::kernel_unavailable(format!("budget store: {e}"))
}

/// `"12.5"` -> 12_500_000. Non-negative, at most six decimals, no signs or exponents.
pub fn parse_micros(s: &str) -> Option<i64> {
    let s = s.trim();
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() || whole.len() > 12 || frac.len() > 6 {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let frac_micros: i64 = format!("{frac:0<6}").parse().ok()?;
    whole.parse::<i64>().ok()?.checked_mul(MICROS)?.checked_add(frac_micros)
}

pub fn fmt_micros(m: i64) -> String {
    let (w, f) = (m / MICROS, m % MICROS);
    if f == 0 {
        w.to_string()
    } else {
        format!("{w}.{f:06}").trim_end_matches('0').to_string()
    }
}

/// The principal's monthly limit in millionths of a USD, from the existing spend limit. `None`
/// means no limit is set (or it is not USD).
fn monthly_limit_micros(conn: &Connection, owner: &str, principal: &str, currency: &str) -> Result<Option<i64>, AuthorityError> {
    if !currency.eq_ignore_ascii_case("usd") {
        return Ok(None);
    }
    let bot = principal.split_once(':').map_or(principal, |(_, id)| id);
    let usd: Option<f64> = conn
        .query_row(
            "SELECT json_extract(config, '$.spendLimit.monthlyUsd') FROM agents WHERE id = ?1 AND user_id = ?2",
            params![bot, owner],
            |r| r.get(0),
        )
        .optional()
        .map_err(store)?
        .flatten();
    Ok(usd.filter(|v| v.is_finite() && *v >= 0.0).map(|v| (v * MICROS as f64).round() as i64))
}

fn month_start(now: DateTime<Utc>) -> String {
    tokens::ts(Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0).single().unwrap_or(now))
}

/// Committed plus live (unexpired, uncommitted) reservations this month for the principal.
fn used_micros(conn: &Connection, owner: &str, principal: &str, currency: &str, now: DateTime<Utc>) -> Result<i64, AuthorityError> {
    conn.query_row(
        "SELECT COALESCE(SUM(CASE WHEN status = 'committed' THEN committed_micros ELSE reserved_micros END), 0)
         FROM kernel_budget_reservations
         WHERE owner_user_id = ?1 AND principal = ?2 AND currency = ?3 AND created_at >= ?4
           AND (status = 'committed' OR (status = 'reserved' AND expires_at > ?5))",
        params![owner, principal, currency, month_start(now), tokens::ts(now)],
        |r| r.get(0),
    )
    .map_err(store)
}

struct Row {
    reservation_id: String,
    run_id: String,
    principal: String,
    scope: BudgetScope,
    currency: String,
    reserved: i64,
    committed: Option<i64>,
    status: BudgetStatus,
    expires_at: String,
    placement_id: Option<String>,
    resource_id: Option<String>,
    usage: Option<BudgetUsage>,
    receipt_id: String,
    idempotency_key: Option<String>,
}

fn enum_from<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(Value::String(s.to_string())).ok()
}

fn load(conn: &Connection, owner: &str, id: &str) -> Result<Option<Row>, AuthorityError> {
    conn.query_row(
        "SELECT reservation_id, run_id, principal, scope, currency, reserved_micros, committed_micros, status, expires_at,
                placement_id, resource_id, usage, receipt_id, idempotency_key
         FROM kernel_budget_reservations WHERE reservation_id = ?1 AND owner_user_id = ?2",
        params![id, owner],
        |r| {
            let scope: String = r.get(3)?;
            let status: String = r.get(7)?;
            let usage: Option<String> = r.get(11)?;
            Ok(Row {
                reservation_id: r.get(0)?,
                run_id: r.get(1)?,
                principal: r.get(2)?,
                scope: enum_from(&scope).unwrap_or(BudgetScope::Month),
                currency: r.get(4)?,
                reserved: r.get(5)?,
                committed: r.get(6)?,
                status: enum_from(&status).unwrap_or(BudgetStatus::Expired),
                expires_at: r.get(8)?,
                placement_id: r.get(9)?,
                resource_id: r.get(10)?,
                usage: usage.and_then(|u| serde_json::from_str(&u).ok()),
                receipt_id: r.get(12)?,
                idempotency_key: r.get(13)?,
            })
        },
    )
    .optional()
    .map_err(store)
}

fn view(r: Row) -> BudgetReservationV1 {
    BudgetReservationV1 {
        schema_id: BUDGET_SCHEMA_ID.into(),
        schema_version: tokens::SCHEMA_VERSION.into(),
        reservation_id: Some(r.reservation_id),
        run_id: r.run_id,
        principal: r.principal,
        scope: r.scope,
        amount_reserved: fmt_micros(r.reserved),
        currency: r.currency,
        status: Some(r.status),
        expires_at: r.expires_at,
        amount_committed: r.committed.map(fmt_micros),
        placement_id: r.placement_id,
        resource_id: r.resource_id,
        usage: r.usage,
        receipt_id: Some(r.receipt_id),
        extensions: None,
    }
}

struct Ctx<'a> {
    owner: &'a str,
    tenant: &'a str,
    signer: &'a allternit_factory_engine::receipts::sign::ReceiptSigner,
    now: DateTime<Utc>,
}

impl Ctx<'_> {
    fn input<'b>(&'b self, run_id: &'b str, decision: &'b str, payload: Value, key: Option<&'b str>) -> ReceiptInput<'b> {
        ReceiptInput {
            run_id,
            principal: self.owner,
            tenant: self.tenant,
            kind: ReceiptKind::Budget,
            schema_id: BUDGET_SCHEMA_ID,
            decision,
            payload,
            idempotency_key: key,
        }
    }

    /// A refusal: its own receipt (the transaction that would have changed state is dropped).
    fn deny(&self, db: &crate::db::DbHandle, run_id: &str, code: ErrorCode, msg: String, payload: Value) -> AuthorityError {
        let written = db.connect().map_err(store).and_then(|mut c| append(&mut c, self.signer, self.input(run_id, "denied", payload, None), self.now));
        match written {
            Ok(rid) => AuthorityError::new(code, msg, Some(rid)),
            Err(e) => e,
        }
    }
}

/// `budget.reserve`.
pub fn reserve(k: &SqlAuthority, caller: &Caller, body: BudgetReservationV1) -> Result<BudgetReservationV1, AuthorityError> {
    let (db, signer, tenant, now) = k.parts();
    let cx = Ctx { owner: &caller.principal, tenant: &tenant, signer: &signer, now };
    if body.schema_id != BUDGET_SCHEMA_ID {
        return Err(bad(format!("schema_id must be {BUDGET_SCHEMA_ID}")));
    }
    if body.run_id.trim().is_empty() || body.principal.trim().is_empty() || body.currency.trim().is_empty() {
        return Err(bad("run_id, principal and currency are required"));
    }
    let amount = parse_micros(&body.amount_reserved).filter(|a| *a > 0).ok_or_else(|| bad("amount_reserved must be a positive decimal string (at most 6 decimals)"))?;
    let expires = tokens::parse_ts(&body.expires_at).filter(|e| *e > now).ok_or_else(|| bad("expires_at must be a future RFC 3339 time"))?;
    let currency = body.currency.trim().to_ascii_uppercase();

    let mut conn = db.connect().map_err(store)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(store)?;
    tx.execute(
        "UPDATE kernel_budget_reservations SET status = 'expired', resolved_at = ?2
         WHERE owner_user_id = ?1 AND status = 'reserved' AND expires_at <= ?2",
        params![cx.owner, tokens::ts(now)],
    )
    .map_err(store)?;
    let limit = monthly_limit_micros(&tx, cx.owner, &body.principal, &currency)?;
    let used = used_micros(&tx, cx.owner, &body.principal, &currency, now)?;
    if let Some(limit) = limit.filter(|l| used + amount > *l) {
        drop(tx);
        let msg = format!("Reserving {} {currency} would pass the limit ({} of {} used).", fmt_micros(amount), fmt_micros(used), fmt_micros(limit));
        return Err(cx.deny(
            &db,
            &body.run_id,
            ErrorCode::BudgetExceeded,
            msg,
            json!({ "event": "reserve", "reason": "budget_exceeded", "principal": body.principal, "amount": fmt_micros(amount),
                    "currency": currency, "used": fmt_micros(used), "limit": fmt_micros(limit), "scope": body.scope }),
        ));
    }
    let id = format!("bud_{}", uuid::Uuid::new_v4().simple());
    let receipt_id = append_in_tx(
        &tx,
        &signer,
        cx.input(
            &body.run_id,
            "reserve",
            json!({ "event": "reserve", "reservation_id": id, "principal": body.principal, "amount": fmt_micros(amount),
                    "currency": currency, "scope": body.scope, "limit": limit.map(fmt_micros), "used": fmt_micros(used),
                    "expires_at": tokens::ts(expires) }),
            None,
        ),
        now,
    )?;
    tx.execute(
        "INSERT INTO kernel_budget_reservations (reservation_id, owner_user_id, tenant, run_id, principal, scope, currency,
            reserved_micros, status, expires_at, created_at, placement_id, resource_id, receipt_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'reserved', ?9, ?10, ?11, ?12, ?13)",
        params![
            id, cx.owner, tenant, body.run_id, body.principal, serde_json::to_value(body.scope).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "month".into()),
            currency, amount, tokens::ts(expires), tokens::ts(now), body.placement_id, body.resource_id, receipt_id
        ],
    )
    .map_err(store)?;
    tx.commit().map_err(store)?;
    load(&conn, cx.owner, &id)?.map(view).ok_or_else(|| store("reservation vanished"))
}

/// `budget.commit`: record the actual amount and release the unused remainder.
pub fn commit(k: &SqlAuthority, caller: &Caller, body: BudgetCommitBody) -> Result<BudgetReservationV1, AuthorityError> {
    let (db, signer, tenant, now) = k.parts();
    let cx = Ctx { owner: &caller.principal, tenant: &tenant, signer: &signer, now };
    let actual = parse_micros(&body.actual_amount).ok_or_else(|| bad("actual_amount must be a non-negative decimal string (at most 6 decimals)"))?;
    if body.idempotency_key.trim().is_empty() {
        return Err(bad("idempotency_key is required"));
    }
    let mut conn = db.connect().map_err(store)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(store)?;
    let Some(row) = load(&tx, cx.owner, &body.reservation_id)? else {
        drop(tx);
        return Err(cx.deny(&db, "budget", ErrorCode::NotFound, "no such reservation".into(), json!({ "event": "commit", "reservation_id": body.reservation_id })));
    };
    if row.status == BudgetStatus::Committed && row.idempotency_key.as_deref() == Some(body.idempotency_key.as_str()) {
        return Ok(view(row));
    }
    let live = row.status == BudgetStatus::Reserved && tokens::parse_ts(&row.expires_at).is_some_and(|e| e > now);
    if !live {
        let (run, id) = (row.run_id.clone(), row.reservation_id.clone());
        drop(tx);
        return Err(cx.deny(&db, &run, ErrorCode::Conflict, "reservation is already resolved or expired".into(), json!({ "event": "commit", "reservation_id": id })));
    }
    if actual > row.reserved {
        let used = used_micros(&tx, cx.owner, &row.principal, &row.currency, now)?;
        if let Some(limit) = monthly_limit_micros(&tx, cx.owner, &row.principal, &row.currency)?.filter(|l| used - row.reserved + actual > *l) {
            let (run, id) = (row.run_id.clone(), row.reservation_id.clone());
            drop(tx);
            return Err(cx.deny(
                &db,
                &run,
                ErrorCode::BudgetExceeded,
                format!("Actual {} is over the reservation and would pass the limit {}.", fmt_micros(actual), fmt_micros(limit)),
                json!({ "event": "commit", "reason": "budget_exceeded", "reservation_id": id, "actual": fmt_micros(actual), "limit": fmt_micros(limit) }),
            ));
        }
    }
    let usage = BudgetUsage { event_type: "usage".into(), quantity: body.quantity, unit: body.unit.clone(), measured_at: tokens::ts(now) };
    let rid = append_in_tx(
        &tx,
        &signer,
        cx.input(
            &row.run_id,
            "commit",
            json!({ "event": "commit", "reservation_id": row.reservation_id, "reserved": fmt_micros(row.reserved), "actual": fmt_micros(actual),
                    "released": fmt_micros((row.reserved - actual).max(0)), "currency": row.currency, "unit": body.unit, "quantity": body.quantity }),
            Some(&body.idempotency_key),
        ),
        now,
    )?;
    tx.execute(
        "UPDATE kernel_budget_reservations SET status = 'committed', committed_micros = ?2, resolved_at = ?3, usage = ?4,
            receipt_id = ?5, idempotency_key = ?6 WHERE reservation_id = ?1",
        params![row.reservation_id, actual, tokens::ts(now), serde_json::to_string(&usage).map_err(store)?, rid, body.idempotency_key],
    )
    .map_err(store)?;
    tx.commit().map_err(store)?;
    load(&conn, cx.owner, &body.reservation_id)?.map(view).ok_or_else(|| store("reservation vanished"))
}

/// `budget.release`: give the reservation back without spending.
pub fn release(k: &SqlAuthority, caller: &Caller, body: BudgetReleaseBody) -> Result<BudgetReservationV1, AuthorityError> {
    let (db, signer, tenant, now) = k.parts();
    let cx = Ctx { owner: &caller.principal, tenant: &tenant, signer: &signer, now };
    let mut conn = db.connect().map_err(store)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(store)?;
    let Some(row) = load(&tx, cx.owner, &body.reservation_id)? else {
        drop(tx);
        return Err(cx.deny(&db, "budget", ErrorCode::NotFound, "no such reservation".into(), json!({ "event": "release", "reservation_id": body.reservation_id })));
    };
    if row.status == BudgetStatus::Released {
        return Ok(view(row));
    }
    if row.status != BudgetStatus::Reserved {
        let (run, id) = (row.run_id.clone(), row.reservation_id.clone());
        drop(tx);
        return Err(cx.deny(&db, &run, ErrorCode::Conflict, "reservation is already committed or expired".into(), json!({ "event": "release", "reservation_id": id })));
    }
    let rid = append_in_tx(
        &tx,
        &signer,
        cx.input(&row.run_id, "release", json!({ "event": "release", "reservation_id": row.reservation_id, "released": fmt_micros(row.reserved), "currency": row.currency }), None),
        now,
    )?;
    tx.execute(
        "UPDATE kernel_budget_reservations SET status = 'released', resolved_at = ?2, receipt_id = ?3 WHERE reservation_id = ?1",
        params![row.reservation_id, tokens::ts(now), rid],
    )
    .map_err(store)?;
    tx.commit().map_err(store)?;
    load(&conn, cx.owner, &body.reservation_id)?.map(view).ok_or_else(|| store("reservation vanished"))
}

// ─── HTTP: POST /api/v1/kernel/budget/{reserve,commit,release} ──────────────────

macro_rules! budget_handler {
    ($name:ident, $body:ty, $call:ident) => {
        pub async fn $name(
            axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
            axum::Extension(user): axum::Extension<crate::auth::AuthUser>,
            credential: Option<axum::Extension<crate::enterprise_auth::CredentialContext>>,
            headers: axum::http::HeaderMap,
            axum::Json(body): axum::Json<Value>,
        ) -> axum::response::Response {
            let body: $body = match super::routes::parse(body) {
                Ok(b) => b,
                Err(r) => return r,
            };
            let caller = super::routes::caller_for(&state, &headers, &user, credential.is_some()).await;
            super::routes::with_kernel(&state, user.tenant_id.clone(), move |k| $call(&k, &caller, body)).await
        }
    };
}

budget_handler!(reserve_handler, BudgetReservationV1, reserve);
budget_handler!(commit_handler, BudgetCommitBody, commit);
budget_handler!(release_handler, BudgetReleaseBody, release);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use allternit_authority::CallerKind;
    use allternit_factory_engine::receipts::sign::ReceiptSigner;
    use std::sync::Arc;

    const USER: &str = "user_b";

    fn t0() -> DateTime<Utc> {
        tokens::parse_ts("2026-11-28T12:00:00Z").unwrap()
    }
    fn kernel(db: &DbHandle) -> SqlAuthority {
        SqlAuthority::new(db.clone(), Arc::new(ReceiptSigner::from_seed([9u8; 32]))).at(t0())
    }
    fn caller() -> Caller {
        Caller { kind: CallerKind::Run, principal: USER.into() }
    }
    fn bot_with_limit(db: &DbHandle, usd: f64) {
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot_1', ?1, 'b', 'm', 'p', 1, ?2)",
                params![USER, json!({ "spendLimit": { "monthlyUsd": usd } }).to_string()],
            )
            .unwrap();
    }
    fn req(amount: &str) -> BudgetReservationV1 {
        BudgetReservationV1 {
            schema_id: BUDGET_SCHEMA_ID.into(),
            schema_version: tokens::SCHEMA_VERSION.into(),
            reservation_id: None,
            run_id: "run_b".into(),
            principal: "agent:bot_1".into(),
            scope: BudgetScope::Month,
            amount_reserved: amount.into(),
            currency: "USD".into(),
            status: None,
            expires_at: "2026-11-28T13:00:00Z".into(),
            amount_committed: None,
            placement_id: None,
            resource_id: None,
            usage: None,
            receipt_id: None,
            extensions: None,
        }
    }
    fn commit_body(id: &str, actual: &str, key: &str) -> BudgetCommitBody {
        BudgetCommitBody { reservation_id: id.into(), actual_amount: actual.into(), quantity: 1.0, unit: "tokens".into(), idempotency_key: key.into() }
    }
    fn receipts(db: &DbHandle, decision: &str) -> i64 {
        db.connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM kernel_receipts WHERE kind = 'budget' AND decision = ?1", params![decision], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn decimal_strings_round_trip() {
        assert_eq!(parse_micros("12.5"), Some(12_500_000));
        assert_eq!(parse_micros("0.000001"), Some(1));
        assert_eq!(parse_micros("-1"), None);
        assert_eq!(parse_micros("1.0000001"), None);
        assert_eq!(fmt_micros(12_500_000), "12.5");
        assert_eq!(fmt_micros(3_000_000), "3");
    }

    #[test]
    fn reserve_over_the_limit_is_refused_with_a_receipt() {
        let db = DbHandle::new_memory().unwrap();
        bot_with_limit(&db, 10.0);
        let k = kernel(&db);
        let first = reserve(&k, &caller(), req("6")).unwrap();
        assert_eq!(first.status, Some(BudgetStatus::Reserved));
        let e = reserve(&k, &caller(), req("5")).unwrap_err();
        assert_eq!(e.code, ErrorCode::BudgetExceeded);
        assert!(e.receipt_id.is_some());
        assert_eq!(receipts(&db, "denied"), 1);
        assert_eq!(receipts(&db, "reserve"), 1);
        // Nothing was reserved by the refused call.
        assert!(reserve(&k, &caller(), req("4")).is_ok());
    }

    #[test]
    fn commit_lower_than_reserve_releases_the_rest() {
        let db = DbHandle::new_memory().unwrap();
        bot_with_limit(&db, 10.0);
        let k = kernel(&db);
        let r = reserve(&k, &caller(), req("8")).unwrap();
        let id = r.reservation_id.clone().unwrap();
        let c = commit(&k, &caller(), commit_body(&id, "3.25", "idem-1")).unwrap();
        assert_eq!(c.status, Some(BudgetStatus::Committed));
        assert_eq!(c.amount_committed.as_deref(), Some("3.25"));
        // 10 - 3.25 is left: a 6.75 reservation fits, a larger one does not.
        assert!(reserve(&k, &caller(), req("7")).is_err());
        assert!(reserve(&k, &caller(), req("6.75")).is_ok());
        // Same idempotency key returns the same committed row; a second commit is a conflict.
        assert_eq!(commit(&k, &caller(), commit_body(&id, "3.25", "idem-1")).unwrap().status, Some(BudgetStatus::Committed));
        assert_eq!(commit(&k, &caller(), commit_body(&id, "1", "idem-2")).unwrap_err().code, ErrorCode::Conflict);
        assert_eq!(receipts(&db, "commit"), 1);
    }

    #[test]
    fn release_gives_the_whole_reservation_back() {
        let db = DbHandle::new_memory().unwrap();
        bot_with_limit(&db, 5.0);
        let k = kernel(&db);
        let id = reserve(&k, &caller(), req("5")).unwrap().reservation_id.unwrap();
        assert!(reserve(&k, &caller(), req("1")).is_err());
        let r = release(&k, &caller(), BudgetReleaseBody { reservation_id: id }).unwrap();
        assert_eq!(r.status, Some(BudgetStatus::Released));
        assert!(reserve(&k, &caller(), req("5")).is_ok());
    }
}
