//! The AllternitOS authority kernel embedded in cloud-api (Stage 1, plan step 18; PLAN.md
//! decision 3, CONTRACT.md sections 4 and 5).
//!
//! cloud-api hosts the kernel for two jobs:
//!
//! 1. **The approval relay** (`routes::kernel_relay`): requests raised on a person's Desktop or
//!    cloud computer are pushed to their phones, and the phone's decision is forwarded back to
//!    that computer, which verifies the person and signs the token. The relay writes its own
//!    receipts here (`relay_received`, `relay_forwarded`, `relay_refused`).
//! 2. **Chats with no computer**: [`check`] is the policy decision for a tool call cloud-api runs
//!    itself, with a receipt written before the answer (CONTRACT.md 5 rule 1). Mode comes from
//!    `ALLTERNIT_KERNEL_MODE` (`shadow` by default, like step 9 in allternit-api): in shadow the
//!    decision and its receipt are recorded and the call always proceeds; `enforce` refuses an
//!    `ask` or human-only class.
//!
//! **Key.** `ALLTERNIT_KERNEL_SEED`: standard base64 of a 32-byte Ed25519 seed, set in
//! cloud-api's secret env on the host (the same file as `ALLTERNIT_DP_JWT_SEED`), never a file in
//! the repo. No seed means no receipt can be written, so [`check`] answers `kernel_unavailable`
//! (DENY in enforce, logged in shadow) and the relay refuses to forward. This key signs only
//! cloud-api's own receipts; it never signs an approval token for a device (that key stays on the
//! device, CONTRACT.md 3.2).
//!
//! **Receipts** (`kernel_receipts`, migration 171): one hash chain per (principal, tenant),
//! `hash` = `sha256:` + hex SHA-256 over the canonical JSON of
//! `{run_id, idx, domain, schema_id, schema_version, payload, prev_hash}`, `signature` =
//! base64url Ed25519 over `hash`. Append-only by trigger. Appends for one chain are serialised
//! with a transaction-scoped advisory lock.

use allternit_authority::policy::{class_rule, Rule};
use allternit_authority::{ActionClass, AutonomyLevel};
use base64::{engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

pub const SEED_ENV: &str = "ALLTERNIT_KERNEL_SEED";
pub const MODE_ENV: &str = "ALLTERNIT_KERNEL_MODE";
/// `ReceiptChainV1.domain`: one fixed ABI value, as in allternit-api (CONTRACT.md 11.2).
pub const RECEIPT_DOMAIN: &str = "allternit.receipt.v1";
pub const SCHEMA_VERSION: &str = "1.0.0";
pub const GENESIS_HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
pub const DEFAULT_TENANT: &str = "default";

/// cloud-api's kernel key.
pub struct KernelKey {
    signing: SigningKey,
    kid: String,
    public_b64: String,
}

impl KernelKey {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        let public = signing.verifying_key().to_bytes();
        Self { kid: URL_SAFE_NO_PAD.encode(Sha256::digest(public)), public_b64: URL_SAFE_NO_PAD.encode(public), signing }
    }

    /// From `ALLTERNIT_KERNEL_SEED`. Read per call so a missing seed fails closed everywhere.
    pub fn from_env() -> Result<Self, String> {
        let raw = std::env::var(SEED_ENV).map_err(|_| format!("{SEED_ENV} is not set"))?;
        let bytes = STANDARD.decode(raw.trim()).map_err(|_| format!("{SEED_ENV} is not valid base64"))?;
        let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| format!("{SEED_ENV} must decode to 32 bytes"))?;
        Ok(Self::from_seed(&seed))
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    fn sign(&self, msg: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.sign(msg).to_bytes())
    }
}

/// Canonical JSON: object keys sorted, no whitespace. Payloads hold strings, integers, booleans,
/// arrays and objects (no floats), so this matches RFC 8785 for them.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> =
                keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[k.as_str()]))).collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

fn sha256_tagged(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// One receipt to append.
pub struct ReceiptIn<'a> {
    pub run_id: &'a str,
    pub principal: &'a str,
    pub kind: &'a str,
    pub schema_id: &'a str,
    pub decision: &'a str,
    pub payload: Value,
    pub idempotency_key: Option<&'a str>,
}

/// Append one receipt to the principal's chain (tenant `default`). Returns the receipt id. An
/// idempotency key seen before returns the original receipt id.
pub async fn append(db: &PgPool, key: &KernelKey, input: ReceiptIn<'_>) -> Result<String, sqlx::Error> {
    if let Some(k) = input.idempotency_key {
        let seen: Option<String> = sqlx::query_scalar("SELECT receipt_id FROM kernel_receipts WHERE idempotency_key = $1")
            .bind(k)
            .fetch_optional(db)
            .await?;
        if let Some(id) = seen {
            return Ok(id);
        }
    }
    let mut payload = match input.payload {
        Value::Object(m) => m,
        other => {
            let mut m = Map::new();
            m.insert("value".into(), other);
            m
        }
    };
    payload.insert("kind".into(), json!(input.kind));
    payload.insert("decision".into(), json!(input.decision));
    let payload = Value::Object(payload);
    let payload_text = canonical(&payload);

    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("kernel_receipts:{}:{}", input.principal, DEFAULT_TENANT))
        .execute(&mut *tx)
        .await?;
    let head: Option<(i64, String)> = sqlx::query_as(
        "SELECT idx, hash FROM kernel_receipts WHERE principal = $1 AND tenant = $2 ORDER BY idx DESC LIMIT 1",
    )
    .bind(input.principal)
    .bind(DEFAULT_TENANT)
    .fetch_optional(&mut *tx)
    .await?;
    let (idx, prev_hash) = head.map_or((0, GENESIS_HASH.to_string()), |(i, h)| (i + 1, h));
    let hash = sha256_tagged(
        canonical(&json!({
            "run_id": input.run_id, "idx": idx, "domain": RECEIPT_DOMAIN, "schema_id": input.schema_id,
            "schema_version": SCHEMA_VERSION, "payload": payload, "prev_hash": prev_hash,
        }))
        .as_bytes(),
    );
    let signature = key.sign(hash.as_bytes());
    let receipt_id = format!("rcpt_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO kernel_receipts (receipt_id, run_id, principal, tenant, idx, domain, kind, schema_id, schema_version,
             payload, decision, prev_hash, hash, signature, key_id, signer_pub, idempotency_key, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
    )
    .bind(&receipt_id)
    .bind(input.run_id)
    .bind(input.principal)
    .bind(DEFAULT_TENANT)
    .bind(idx)
    .bind(RECEIPT_DOMAIN)
    .bind(input.kind)
    .bind(input.schema_id)
    .bind(SCHEMA_VERSION)
    .bind(&payload_text)
    .bind(input.decision)
    .bind(&prev_hash)
    .bind(&hash)
    .bind(&signature)
    .bind(key.kid())
    .bind(&key.public_b64)
    .bind(input.idempotency_key)
    .bind(Utc::now())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(receipt_id)
}

/// `shadow` (default) or `enforce`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Shadow,
    Enforce,
}

pub fn mode() -> Mode {
    match std::env::var(MODE_ENV).as_deref().map(str::trim) {
        Ok("enforce") => Mode::Enforce,
        _ => Mode::Shadow,
    }
}

/// What [`check`] says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// The policy answer: `allow`, `ask`, `human_only` or `kernel_unavailable`.
    pub decision: &'static str,
    /// Whether the caller may run the tool now. Always true in shadow mode.
    pub proceed: bool,
    pub receipt_id: Option<String>,
}

/// `check` for a tool call in a chat with no computer. No approval token can exist for these yet
/// (the phone relay resolves device-held requests), so `ask` and `human_only` are refused in
/// enforce mode and only recorded in shadow mode. Autonomy is `ask_first` (no consent receipt
/// exists for cloud chats, CONTRACT.md 4.4).
pub async fn check(db: &PgPool, principal: &str, run_id: &str, tool: &str, class: ActionClass) -> Checked {
    check_in(db, KernelKey::from_env(), mode(), principal, run_id, tool, class).await
}

/// [`check`] with the key and mode supplied (tests).
pub async fn check_in(
    db: &PgPool,
    key: Result<KernelKey, String>,
    mode: Mode,
    principal: &str,
    run_id: &str,
    tool: &str,
    class: ActionClass,
) -> Checked {
    let decision = match class_rule(class, AutonomyLevel::AskFirst) {
        Rule::Allow => "allow",
        Rule::Ask => "ask",
        Rule::HumanOnly => "human_only",
    };
    let class_name = serde_json::to_value(class).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let mode_name = if mode == Mode::Shadow { "shadow" } else { "enforce" };
    let receipt = match key {
        Ok(key) => append(
            db,
            &key,
            ReceiptIn {
                run_id,
                principal,
                kind: "check",
                schema_id: "allternit.os.PolicyCheck",
                decision,
                payload: json!({ "tool": tool, "action_class": class_name, "autonomy_level": "ask_first", "mode": mode_name, "surface": "cloud_chat" }),
                idempotency_key: None,
            },
        )
        .await
        .map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    match receipt {
        Ok(id) => {
            let proceed = mode == Mode::Shadow || decision == "allow";
            if decision != "allow" {
                tracing::info!(tool, class = %class_name, decision, mode = mode_name, "kernel check (cloud chat)");
            }
            Checked { decision, proceed, receipt_id: Some(id) }
        }
        // Write before act: no receipt means DENY when enforcing.
        Err(e) => {
            tracing::warn!(tool, "kernel check: receipt not written ({e})");
            Checked { decision: "kernel_unavailable", proceed: mode == Mode::Shadow, receipt_id: None }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use std::sync::Arc;

    pub(crate) async fn receipts_schema(db: &PgPool) {
        sqlx::raw_sql(include_str!("../migrations_pg/171_kernel_approval_relay.sql")).execute(db).await.unwrap();
    }

    pub(crate) fn test_key() -> KernelKey {
        KernelKey::from_seed(&[7u8; 32])
    }

    #[test]
    fn canonical_sorts_keys_at_every_depth() {
        let v = json!({ "b": 1, "a": { "d": [true, null], "c": "x" } });
        assert_eq!(canonical(&v), r#"{"a":{"c":"x","d":[true,null]},"b":1}"#);
    }

    #[tokio::test]
    async fn shadow_records_but_proceeds_and_enforce_refuses_a_human_only_class() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        receipts_schema(&state.db).await;
        let shadow = check_in(&state.db, Ok(test_key()), Mode::Shadow, "user_1", "chat_1", "send_invoice", ActionClass::Money).await;
        assert_eq!((shadow.decision, shadow.proceed), ("human_only", true));
        assert!(shadow.receipt_id.is_some());
        let enforce = check_in(&state.db, Ok(test_key()), Mode::Enforce, "user_1", "chat_1", "send_invoice", ActionClass::Money).await;
        assert_eq!((enforce.decision, enforce.proceed), ("human_only", false));
        let read = check_in(&state.db, Ok(test_key()), Mode::Enforce, "user_1", "chat_1", "web_search", ActionClass::Read).await;
        assert_eq!((read.decision, read.proceed), ("allow", true));
        // No key: no receipt, so enforce denies.
        let nokey = check_in(&state.db, Err("unset".into()), Mode::Enforce, "user_1", "chat_1", "web_search", ActionClass::Read).await;
        assert_eq!((nokey.decision, nokey.proceed), ("kernel_unavailable", false));
        // One chain, linked, and the table refuses edits.
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT idx, prev_hash, hash FROM kernel_receipts WHERE principal = 'user_1' ORDER BY idx").fetch_all(&state.db).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, GENESIS_HASH);
        assert_eq!(rows[1].1, rows[0].2);
        assert_eq!(rows[2].1, rows[1].2);
        assert!(sqlx::query("UPDATE kernel_receipts SET decision = 'allow'").execute(&state.db).await.is_err());
        assert!(sqlx::query("DELETE FROM kernel_receipts").execute(&state.db).await.is_err());
    }
}
