//! AllternitOS authority kernel, hosted in allternit-api (Stage 1, plan step 7).
//!
//! Implements the `allternit-authority` crate's approval calls over SQLite (migration V300):
//! `approval.request`, `approval.resolve` (human session only) and `approval.redeem`.
//! Spec: `handoffs/allternit-os-2026-11-27/CONTRACT.md` sections 2.4, 3.1, 3.2, 4.3, 5, 11, 12.
//!
//! | Module | What |
//! |---|---|
//! | [`tokens`] | `ApprovalTokenV1`: Ed25519 mint/verify, TTL 300 s default, 900 s cap, the kernel key |
//! | [`approvals`] | [`approvals::SqlAuthority`]: request, resolve, redeem; receipts written before acting |
//! | [`routes`] | `POST /api/v1/kernel/approvals/{request,resolve,redeem}`; resolve needs a person's session |
//!
//! Not here yet: `check` at the choke points (step 9), `secret.use` (10), moving the nine old
//! approval stores onto this one (11), budgets and leases (12), full chain verification (8).

pub mod approvals;
pub mod routes;
pub mod tokens;

use allternit_authority::{AuthorityError, ReceiptKind};
use allternit_factory_engine::receipts::jcs;
use allternit_factory_engine::receipts::sign::ReceiptSigner;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

/// `ReceiptChainV1.domain`: one fixed ABI value; the kernel kind rides in `payload.kind` (CONTRACT.md 11.2).
pub const RECEIPT_DOMAIN: &str = "allternit.receipt.v1";
/// `prev_hash` of the first receipt in a chain.
pub const GENESIS_HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// One receipt to append.
pub struct ReceiptInput<'a> {
    /// Chain key (`kernel_receipts.run_id`).
    pub chain: &'a str,
    pub kind: ReceiptKind,
    pub schema_id: &'a str,
    /// Must be a JSON object; `kind` is set from [`ReceiptInput::kind`]. Never a secret value.
    pub payload: Value,
    pub idempotency_key: Option<&'a str>,
}

fn kind_str(k: ReceiptKind) -> String {
    serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Append one signed, hash-chained row to `kernel_receipts` and return its `receipt_id`.
///
/// Minimal helper for step 7 (write-before-act). Call it inside the same transaction as the
/// state change it records, before that change. Two writers racing on one chain collide on
/// `UNIQUE (run_id, idx)` and the loser gets `kernel_unavailable` (fail closed). Step 8 adds
/// `verify_chain` and refuses to extend a broken chain.
pub fn append_receipt(
    conn: &Connection,
    signer: &ReceiptSigner,
    input: ReceiptInput<'_>,
    now: DateTime<Utc>,
) -> Result<String, AuthorityError> {
    let fail = |what: &str, e: &dyn std::fmt::Display| AuthorityError::kernel_unavailable(format!("receipt {what}: {e}"));
    let mut payload = input.payload;
    let Some(obj) = payload.as_object_mut() else {
        return Err(AuthorityError::kernel_unavailable("receipt payload must be an object"));
    };
    let kind = kind_str(input.kind);
    obj.insert("kind".into(), json!(kind));

    let last: Option<(i64, String)> = conn
        .query_row(
            "SELECT idx, hash FROM kernel_receipts WHERE run_id = ?1 ORDER BY idx DESC LIMIT 1",
            params![input.chain],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| fail("read chain head", &e))?;
    let (idx, prev_hash) = match last {
        Some((i, h)) => (i + 1, h),
        None => (0, GENESIS_HASH.to_string()),
    };

    let schema_version = tokens::SCHEMA_VERSION;
    let hashed = json!({
        "run_id": input.chain,
        "idx": idx,
        "domain": RECEIPT_DOMAIN,
        "schema_id": input.schema_id,
        "schema_version": schema_version,
        "payload": payload,
        "prev_hash": prev_hash,
    });
    let hash = jcs::hash_value(&hashed).map_err(|e| fail("hash", &e))?;
    let signature = signer.sign(hash.as_bytes());
    let payload_text = jcs::canonicalize(&payload).map_err(|e| fail("canonical payload", &e))?;
    let receipt_id = format!("rcpt_{}", uuid::Uuid::new_v4().simple());

    conn.execute(
        "INSERT INTO kernel_receipts (receipt_id, run_id, idx, domain, kind, schema_id, schema_version,
             payload, prev_hash, hash, signature, key_id, idempotency_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            receipt_id,
            input.chain,
            idx,
            RECEIPT_DOMAIN,
            kind,
            input.schema_id,
            schema_version,
            payload_text,
            prev_hash,
            hash,
            signature,
            signer.kid(),
            input.idempotency_key,
            tokens::ts(now),
        ],
    )
    .map_err(|e| fail("insert", &e))?;
    Ok(receipt_id)
}
