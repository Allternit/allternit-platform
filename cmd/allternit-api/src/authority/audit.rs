//! The kernel receipt log: hash-chained, signed, append-only (CONTRACT.md 5, 11.2; plan step 8).
//!
//! * **Scope.** One chain per `(principal, tenant)`, never per kind or per run. `run_id` stays on
//!   the row as a column. `idx` starts at 0 in each chain.
//! * **Hash.** `sha256:<hex>` over the RFC 8785 canonical form of `{prev_hash, kind, principal,
//!   tenant, payload_hash, decision, created_at}` plus the row's position and schema fields
//!   (`run_id, idx, domain, schema_id, schema_version`), so a row cannot be moved or relabelled.
//! * **Signature.** Ed25519 over the hash with the kernel key. Each row stores the signer's key id
//!   and public key, so a later key change reads "signed by a previous key", not "tampered".
//! * **Append.** [`append`] runs in one `BEGIN IMMEDIATE` transaction that reads the chain head,
//!   so two writers form a line. [`append_in_tx`] is the same step for a caller that already
//!   holds such a transaction and wants the receipt in the same commit as its state change
//!   (write before act). It first re-checks the head row and refuses to extend a broken chain.
//! * **Verify.** [`verify_chain`] recomputes every hash, link and signature in order and returns
//!   the first bad row.

use allternit_authority::{AuthorityError, ReceiptKind};
use allternit_factory_engine::receipts::jcs;
use allternit_factory_engine::receipts::sign::{kid_for, verify_sig, ReceiptSigner};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Value};

use super::{tokens, GENESIS_HASH, RECEIPT_DOMAIN};

/// Tenant used when the account has none (single-user Desktop, local dev).
pub const DEFAULT_TENANT: &str = "default";

/// One receipt to append.
pub struct ReceiptInput<'a> {
    /// `kernel_receipts.run_id` (a column, not the chain key).
    pub run_id: &'a str,
    /// Chain scope, with `tenant`.
    pub principal: &'a str,
    pub tenant: &'a str,
    pub kind: ReceiptKind,
    pub schema_id: &'a str,
    /// What the kernel decided: `request`, `approve`, `deny`, `denied`, `redeem`, `expired`, ...
    pub decision: &'a str,
    /// Must be a JSON object; `kind` is set from [`ReceiptInput::kind`]. Never a secret value.
    pub payload: Value,
    pub idempotency_key: Option<&'a str>,
}

/// The chain a row belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub principal: String,
    pub tenant: String,
}

fn fail(what: &str, e: &dyn std::fmt::Display) -> AuthorityError {
    AuthorityError::kernel_unavailable(format!("receipt {what}: {e}"))
}

pub fn kind_str(k: ReceiptKind) -> String {
    serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Every field the hash covers, as read from or written to a row.
struct Fields<'a> {
    prev_hash: &'a str,
    kind: &'a str,
    principal: &'a str,
    tenant: &'a str,
    payload_hash: &'a str,
    decision: &'a str,
    created_at: &'a str,
    run_id: &'a str,
    idx: i64,
    domain: &'a str,
    schema_id: &'a str,
    schema_version: &'a str,
}

fn row_hash(f: &Fields<'_>) -> Result<String, String> {
    jcs::hash_value(&json!({
        "prev_hash": f.prev_hash, "kind": f.kind, "principal": f.principal, "tenant": f.tenant,
        "payload_hash": f.payload_hash, "decision": f.decision, "created_at": f.created_at,
        "run_id": f.run_id, "idx": f.idx, "domain": f.domain,
        "schema_id": f.schema_id, "schema_version": f.schema_version,
    }))
    .map_err(|e| e.to_string())
}

/// Append in its own `BEGIN IMMEDIATE` transaction (denials and other standalone receipts).
pub fn append(
    conn: &mut Connection,
    signer: &ReceiptSigner,
    input: ReceiptInput<'_>,
    now: DateTime<Utc>,
) -> Result<String, AuthorityError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| fail("begin", &e))?;
    let id = append_in_tx(&tx, signer, input, now)?;
    tx.commit().map_err(|e| fail("commit", &e))?;
    Ok(id)
}

/// Append inside a transaction the caller opened with `TransactionBehavior::Immediate` (so the
/// chain head read and the insert cannot interleave with another writer). Call it before the
/// state change it records, then commit both together.
pub fn append_in_tx(
    conn: &Connection,
    signer: &ReceiptSigner,
    input: ReceiptInput<'_>,
    now: DateTime<Utc>,
) -> Result<String, AuthorityError> {
    let mut payload = input.payload;
    let Some(obj) = payload.as_object_mut() else {
        return Err(AuthorityError::kernel_unavailable("receipt payload must be an object"));
    };
    let kind = kind_str(input.kind);
    obj.insert("kind".into(), json!(kind));

    // Head of this chain; refuse to extend it if the head row no longer verifies.
    let head: Option<Stored> = conn
        .query_row(&format!("{SELECT_ROW} WHERE principal = ?1 AND tenant = ?2 ORDER BY idx DESC LIMIT 1"), params![input.principal, input.tenant], Stored::read)
        .optional()
        .map_err(|e| fail("read chain head", &e))?;
    let (idx, prev_hash) = match head {
        Some(h) => {
            if let Some(bad) = check_row(&h, None).err() {
                return Err(AuthorityError::new(
                    allternit_authority::ErrorCode::ChainBroken,
                    format!("receipt chain is broken at row {} ({}); refusing to extend it", h.idx, bad.as_str()),
                    None,
                ));
            }
            (h.idx + 1, h.hash)
        }
        None => (0, GENESIS_HASH.to_string()),
    };

    let schema_version = tokens::SCHEMA_VERSION;
    let created_at = tokens::ts(now);
    let payload_text = jcs::canonicalize(&payload).map_err(|e| fail("canonical payload", &e))?;
    let payload_hash = jcs::sha256_tagged(payload_text.as_bytes());
    let hash = row_hash(&Fields {
        prev_hash: &prev_hash, kind: &kind, principal: input.principal, tenant: input.tenant,
        payload_hash: &payload_hash, decision: input.decision, created_at: &created_at,
        run_id: input.run_id, idx, domain: RECEIPT_DOMAIN, schema_id: input.schema_id, schema_version,
    })
    .map_err(|e| fail("hash", &e))?;
    let signature = signer.sign(hash.as_bytes());
    let signer_pub = B64.encode(signer.verifying_key().as_bytes());
    let receipt_id = format!("rcpt_{}", uuid::Uuid::new_v4().simple());

    conn.execute(
        "INSERT INTO kernel_receipts (receipt_id, run_id, principal, tenant, idx, domain, kind, schema_id, schema_version,
             payload, payload_hash, decision, prev_hash, hash, signature, key_id, signer_pub, idempotency_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            receipt_id, input.run_id, input.principal, input.tenant, idx, RECEIPT_DOMAIN, kind, input.schema_id,
            schema_version, payload_text, payload_hash, input.decision, prev_hash, hash, signature, signer.kid(),
            signer_pub, input.idempotency_key, created_at,
        ],
    )
    .map_err(|e| fail("insert", &e))?;
    Ok(receipt_id)
}

const SELECT_ROW: &str = "SELECT receipt_id, run_id, principal, tenant, idx, domain, kind, schema_id, schema_version, payload,
    payload_hash, decision, prev_hash, hash, signature, key_id, signer_pub, created_at FROM kernel_receipts";

struct Stored {
    receipt_id: String,
    run_id: String,
    principal: String,
    tenant: String,
    idx: i64,
    domain: String,
    kind: String,
    schema_id: String,
    schema_version: String,
    payload: String,
    payload_hash: String,
    decision: String,
    prev_hash: String,
    hash: String,
    signature: String,
    key_id: String,
    signer_pub: String,
    created_at: String,
}

impl Stored {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            receipt_id: r.get(0)?, run_id: r.get(1)?, principal: r.get(2)?, tenant: r.get(3)?, idx: r.get(4)?,
            domain: r.get(5)?, kind: r.get(6)?, schema_id: r.get(7)?, schema_version: r.get(8)?, payload: r.get(9)?,
            payload_hash: r.get(10)?, decision: r.get(11)?, prev_hash: r.get(12)?, hash: r.get(13)?,
            signature: r.get(14)?, key_id: r.get(15)?, signer_pub: r.get(16)?, created_at: r.get(17)?,
        })
    }
}

/// Why a row failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BadReason {
    /// The stored payload no longer matches `payload_hash`.
    PayloadHashMismatch,
    /// Recomputing the row hash gives a different value (a field was edited).
    HashMismatch,
    /// `prev_hash` is not the previous row's hash, or the chain starts wrong.
    BrokenLink,
    /// The signature does not verify against the row's own signer key, or that key does not match `key_id`.
    BadSignature,
}

impl BadReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BadReason::PayloadHashMismatch => "payload_hash_mismatch",
            BadReason::HashMismatch => "hash_mismatch",
            BadReason::BrokenLink => "broken_link",
            BadReason::BadSignature => "bad_signature",
        }
    }
}

/// Check one row on its own. `Ok(true)` = valid and signed by the current key (or `current` is
/// unknown), `Ok(false)` = valid but signed by a previous key.
fn check_row(s: &Stored, current_kid: Option<&str>) -> Result<bool, BadReason> {
    if jcs::sha256_tagged(s.payload.as_bytes()) != s.payload_hash {
        return Err(BadReason::PayloadHashMismatch);
    }
    let want = row_hash(&Fields {
        prev_hash: &s.prev_hash, kind: &s.kind, principal: &s.principal, tenant: &s.tenant,
        payload_hash: &s.payload_hash, decision: &s.decision, created_at: &s.created_at,
        run_id: &s.run_id, idx: s.idx, domain: &s.domain, schema_id: &s.schema_id, schema_version: &s.schema_version,
    })
    .map_err(|_| BadReason::HashMismatch)?;
    if want != s.hash {
        return Err(BadReason::HashMismatch);
    }
    let vk: VerifyingKey = B64
        .decode(&s.signer_pub)
        .ok()
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .and_then(|raw| VerifyingKey::from_bytes(&raw).ok())
        .ok_or(BadReason::BadSignature)?;
    if kid_for(&vk) != s.key_id || !verify_sig(&vk, s.hash.as_bytes(), &s.signature) {
        return Err(BadReason::BadSignature);
    }
    Ok(current_kid.map_or(true, |k| k == s.key_id))
}

/// First bad row of a chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BadRow {
    pub receipt_id: String,
    pub idx: i64,
    pub reason: BadReason,
}

/// Result of [`verify_chain`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChainReport {
    pub ok: bool,
    pub rows: u64,
    pub first_bad: Option<BadRow>,
    /// Rows that verify but were signed by a key other than the current one (kernel key
    /// changed or was replaced). Not an integrity failure.
    pub signed_by_previous_key: u64,
    /// Receipt ids of the first few previous-key rows, so the page can say which.
    pub previous_key_ids: Vec<String>,
}

/// Recompute every hash, link and signature of one chain in order. `current_kid` is the kernel
/// key now in use: rows signed by another key that still verify against the public key stored on
/// the row count as `signed_by_previous_key`, never as tampered. Read-only.
pub fn verify_chain(conn: &Connection, scope: &Scope, current_kid: Option<&str>) -> Result<ChainReport, AuthorityError> {
    let mut stmt = conn
        .prepare(&format!("{SELECT_ROW} WHERE principal = ?1 AND tenant = ?2 ORDER BY idx ASC"))
        .map_err(|e| fail("read chain", &e))?;
    let rows = stmt
        .query_map(params![scope.principal, scope.tenant], Stored::read)
        .map_err(|e| fail("read chain", &e))?;
    let mut report = ChainReport { ok: true, rows: 0, first_bad: None, signed_by_previous_key: 0, previous_key_ids: vec![] };
    let mut expect_prev = GENESIS_HASH.to_string();
    let mut expect_idx = 0i64;
    for row in rows {
        let s = row.map_err(|e| fail("read chain", &e))?;
        report.rows += 1;
        let bad = |reason| BadRow { receipt_id: s.receipt_id.clone(), idx: s.idx, reason };
        let outcome = if s.idx != expect_idx || s.prev_hash != expect_prev {
            Err(BadReason::BrokenLink)
        } else {
            check_row(&s, current_kid)
        };
        match outcome {
            Ok(current) => {
                if !current {
                    report.signed_by_previous_key += 1;
                    if report.previous_key_ids.len() < 5 {
                        report.previous_key_ids.push(s.receipt_id.clone());
                    }
                }
            }
            Err(reason) => {
                report.ok = false;
                report.first_bad = Some(bad(reason));
                return Ok(report);
            }
        }
        expect_prev = s.hash.clone();
        expect_idx += 1;
    }
    Ok(report)
}

/// The caller's own receipts, newest first, for `GET /api/v1/kernel/receipts`.
pub fn list(conn: &Connection, scope: &Scope, limit: u32, before_idx: Option<i64>) -> Result<Vec<Value>, AuthorityError> {
    let mut stmt = conn
        .prepare(&format!("{SELECT_ROW} WHERE principal = ?1 AND tenant = ?2 AND idx < ?3 ORDER BY idx DESC LIMIT ?4"))
        .map_err(|e| fail("list", &e))?;
    let rows = stmt
        .query_map(params![scope.principal, scope.tenant, before_idx.unwrap_or(i64::MAX), limit.min(500)], Stored::read)
        .map_err(|e| fail("list", &e))?;
    let mut out = vec![];
    for r in rows {
        let s = r.map_err(|e| fail("list", &e))?;
        let payload: Value = serde_json::from_str(&s.payload).unwrap_or(Value::Null);
        out.push(json!({
            "receipt_id": s.receipt_id, "run_id": s.run_id, "idx": s.idx, "kind": s.kind, "decision": s.decision,
            "schema_id": s.schema_id, "payload": payload, "payload_hash": s.payload_hash, "prev_hash": s.prev_hash,
            "hash": s.hash, "signature": s.signature, "key_id": s.key_id, "created_at": s.created_at,
        }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use std::sync::Arc;

    fn t0() -> DateTime<Utc> {
        tokens::parse_ts("2026-11-28T12:00:00Z").unwrap()
    }

    fn input<'a>(principal: &'a str, n: u32) -> ReceiptInput<'a> {
        ReceiptInput {
            run_id: "run_1", principal, tenant: "t1", kind: ReceiptKind::Approval, schema_id: "test.schema",
            decision: "request", payload: json!({ "n": n }), idempotency_key: None,
        }
    }

    fn scope(p: &str) -> Scope {
        Scope { principal: p.into(), tenant: "t1".into() }
    }

    fn put(db: &DbHandle, signer: &ReceiptSigner, p: &str, n: u32) -> String {
        let mut c = db.connect().unwrap();
        append(&mut c, signer, input(p, n), t0()).unwrap()
    }

    #[test]
    fn a_clean_chain_verifies_and_chains_per_principal_and_tenant() {
        let db = DbHandle::new_memory().unwrap();
        let signer = ReceiptSigner::from_seed([7u8; 32]);
        for n in 0..3 {
            put(&db, &signer, "u1", n);
        }
        put(&db, &signer, "u2", 0);
        let c = db.connect().unwrap();
        let r = verify_chain(&c, &scope("u1"), Some(signer.kid())).unwrap();
        assert!(r.ok && r.rows == 3 && r.first_bad.is_none() && r.signed_by_previous_key == 0);
        // u2 starts its own chain at idx 0 even though run_id is shared.
        let (idx, prev): (i64, String) = c
            .query_row("SELECT idx, prev_hash FROM kernel_receipts WHERE principal = 'u2'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((idx, prev.as_str()), (0, GENESIS_HASH));
    }

    #[test]
    fn an_edited_row_fails_verification() {
        let db = DbHandle::new_memory().unwrap();
        let signer = ReceiptSigner::from_seed([7u8; 32]);
        let ids: Vec<String> = (0..3).map(|n| put(&db, &signer, "u1", n)).collect();
        let c = db.connect().unwrap();
        // The triggers block UPDATE, so an attacker with file access drops one first.
        c.execute_batch("DROP TRIGGER kernel_receipts_no_update").unwrap();
        c.execute("UPDATE kernel_receipts SET decision = 'approve' WHERE receipt_id = ?1", params![ids[1]]).unwrap();
        let r = verify_chain(&c, &scope("u1"), Some(signer.kid())).unwrap();
        assert!(!r.ok);
        let bad = r.first_bad.unwrap();
        assert_eq!((bad.receipt_id, bad.reason), (ids[1].clone(), BadReason::HashMismatch));
        // And a broken chain is not extended.
        let mut c2 = db.connect().unwrap();
        let tail = ids[2].clone();
        c.execute("UPDATE kernel_receipts SET decision = 'request' WHERE receipt_id = ?1", params![ids[1]]).unwrap();
        c.execute("UPDATE kernel_receipts SET payload = '{}' WHERE receipt_id = ?1", params![tail]).unwrap();
        let err = append(&mut c2, &signer, input("u1", 9), t0()).unwrap_err();
        assert_eq!(err.code, allternit_authority::ErrorCode::ChainBroken);
    }

    #[test]
    fn a_deleted_row_is_refused_by_the_trigger() {
        let db = DbHandle::new_memory().unwrap();
        let signer = ReceiptSigner::from_seed([7u8; 32]);
        let id = put(&db, &signer, "u1", 0);
        let c = db.connect().unwrap();
        assert!(c.execute("DELETE FROM kernel_receipts WHERE receipt_id = ?1", params![id]).is_err());
        assert!(c.execute("UPDATE kernel_receipts SET decision = 'x'", []).is_err());
    }

    #[test]
    fn two_racing_appends_yield_a_linear_chain() {
        let db = Arc::new(DbHandle::new_memory().unwrap());
        let signer = Arc::new(ReceiptSigner::from_seed([7u8; 32]));
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let (db, signer) = (db.clone(), signer.clone());
                std::thread::spawn(move || put(&db, &signer, "u1", n))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let c = db.connect().unwrap();
        let r = verify_chain(&c, &scope("u1"), Some(signer.kid())).unwrap();
        assert!(r.ok && r.rows == 8, "{r:?}");
        let max: i64 = c.query_row("SELECT MAX(idx) FROM kernel_receipts WHERE principal = 'u1'", [], |r| r.get(0)).unwrap();
        assert_eq!(max, 7);
    }

    #[test]
    fn a_key_change_reports_previous_key_not_tampered() {
        let db = DbHandle::new_memory().unwrap();
        let old = ReceiptSigner::from_seed([7u8; 32]);
        let new = ReceiptSigner::from_seed([8u8; 32]);
        put(&db, &old, "u1", 0);
        put(&db, &new, "u1", 1);
        let c = db.connect().unwrap();
        let r = verify_chain(&c, &scope("u1"), Some(new.kid())).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.signed_by_previous_key, 1);
        // A forged signature (right key id, wrong signature) is still tampered.
        c.execute_batch("DROP TRIGGER kernel_receipts_no_update").unwrap();
        let forged = new.sign(b"something else");
        c.execute("UPDATE kernel_receipts SET signature = ?1 WHERE idx = 0", params![forged]).unwrap();
        let r = verify_chain(&c, &scope("u1"), Some(new.kid())).unwrap();
        assert_eq!(r.first_bad.unwrap().reason, BadReason::BadSignature);
    }
}
