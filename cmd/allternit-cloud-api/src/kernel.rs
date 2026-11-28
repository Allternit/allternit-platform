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
    #[cfg(test)]
    if let Some(m) = TEST_KERNEL.with(|k| k.get()) {
        return m;
    }
    match std::env::var(MODE_ENV).as_deref().map(str::trim) {
        Ok("enforce") => Mode::Enforce,
        _ => Mode::Shadow,
    }
}

/// One server-side tool call to check. Built by the path that runs the tool (never from model
/// output), classified by [`crate::kernel_tools`].
#[derive(Clone, Debug)]
pub struct Tool<'a> {
    /// Which cloud-api path runs it (`mcp_edge`, `meeting_pod`, `agent_tools`).
    pub surface: &'a str,
    pub name: &'a str,
    /// The arguments the tool runs with. Hashed into `action_hash`, never written to a receipt.
    pub args: &'a Value,
    pub class: ActionClass,
    /// `false` = the tool is in no table: the rule is forced to `ask`, whatever `class` says.
    pub known: bool,
    /// Set when a person already approved this exact call through the path's own flow (the
    /// meeting owner's approval). The receipt records it and the call proceeds.
    pub approved_by: Option<&'a str>,
}

impl Tool<'_> {
    /// `sha256:<hex>` of the canonical `{tool, args}`, the same shape allternit-api binds tokens to.
    pub fn action_hash(&self) -> String {
        sha256_tagged(canonical(&json!({ "tool": self.name, "args": self.args })).as_bytes())
    }
}

#[cfg(test)]
thread_local! {
    /// Tests on this thread: the mode to use, with the test key, instead of the process env.
    static TEST_KERNEL: std::cell::Cell<Option<Mode>> = const { std::cell::Cell::new(None) };
}

/// Run kernel checks on this test thread in `mode` with the test key (the env is process-wide).
#[cfg(test)]
pub(crate) fn set_test_kernel(mode: Option<Mode>) {
    TEST_KERNEL.with(|k| k.set(mode));
}

fn key_from_env() -> Result<KernelKey, String> {
    #[cfg(test)]
    if TEST_KERNEL.with(|k| k.get()).is_some() {
        return Ok(KernelKey::from_seed(&[7u8; 32]));
    }
    KernelKey::from_env()
}

/// What [`check`] says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// The policy answer: `allow`, `ask`, `human_only` or `kernel_unavailable`.
    pub decision: &'static str,
    /// Whether the caller may run the tool now. Always true in shadow mode.
    pub proceed: bool,
    pub receipt_id: Option<String>,
    pub action_hash: String,
    pub action_class: &'static str,
}

impl Checked {
    /// `None` when the tool may run, else the refusal a tool caller gets. cloud-api has no
    /// approval request store for chats with no computer yet, so `approval_request_id` is null
    /// and the caller gets the `action_hash` to approve (the no-computer approval flow is the
    /// next gap).
    pub fn refusal(&self) -> Option<Value> {
        if self.proceed {
            return None;
        }
        if self.decision == "kernel_unavailable" {
            return Some(json!({
                "code": "kernel_unavailable", "error": "kernel_unavailable",
                "message": "The authority kernel could not record this action, so it did not run.",
            }));
        }
        Some(json!({
            "code": "approval_required", "error": "approval_required",
            "message": "This action needs a person's approval before it runs.",
            "approval_request_id": null, "action_hash": self.action_hash, "action_class": self.action_class,
            "receipt_id": self.receipt_id,
        }))
    }
}

/// `check` for a tool call cloud-api runs itself. Call it exactly once per call, before the tool
/// runs, and stop when [`Checked::refusal`] is `Some`. No approval token can exist for these yet
/// (the phone relay resolves device-held requests), so `ask` and `human_only` are refused in
/// enforce mode and only recorded in shadow mode. Autonomy is `ask_first` (no consent receipt
/// exists for cloud chats, CONTRACT.md 4.4).
pub async fn check(db: &PgPool, principal: &str, run_id: &str, tool: &Tool<'_>) -> Checked {
    check_in(db, key_from_env(), mode(), principal, run_id, tool).await
}

/// [`check`] with the key and mode supplied (tests).
pub async fn check_in(db: &PgPool, key: Result<KernelKey, String>, mode: Mode, principal: &str, run_id: &str, tool: &Tool<'_>) -> Checked {
    let decision = if tool.approved_by.is_some() {
        "allow"
    } else if !tool.known {
        "ask"
    } else {
        match class_rule(tool.class, AutonomyLevel::AskFirst) {
            Rule::Allow => "allow",
            Rule::Ask => "ask",
            Rule::HumanOnly => "human_only",
        }
    };
    let class_name = enum_name(tool.class);
    let mode_name = if mode == Mode::Shadow { "shadow" } else { "enforce" };
    let action_hash = tool.action_hash();
    let done = |decision: &'static str, proceed: bool, receipt_id: Option<String>| Checked {
        decision,
        proceed,
        receipt_id,
        action_hash: action_hash.clone(),
        action_class: class_name,
    };
    let key = match key {
        Ok(k) => k,
        Err(e) => {
            // One warning per process, not one per call. Shadow still runs (no receipt).
            static MISSING_KEY: std::sync::Once = std::sync::Once::new();
            MISSING_KEY.call_once(|| tracing::warn!("kernel check: no receipts will be written ({e})"));
            return done("kernel_unavailable", mode == Mode::Shadow, None);
        }
    };
    let mut payload = json!({
        "tool": tool.name, "action_class": class_name, "classified": if tool.known { "table" } else { "default_ask" },
        "autonomy_level": "ask_first", "mode": mode_name, "surface": tool.surface, "action_hash": action_hash,
    });
    if let Some(by) = tool.approved_by {
        payload["approved_by"] = json!(by);
    }
    let receipt = append(
        db,
        &key,
        ReceiptIn { run_id, principal, kind: "check", schema_id: "allternit.os.PolicyCheck", decision, payload, idempotency_key: None },
    )
    .await;
    match receipt {
        Ok(id) => {
            if decision != "allow" {
                tracing::info!(tool = tool.name, class = %class_name, decision, mode = mode_name, "kernel check (cloud)");
            }
            done(decision, mode == Mode::Shadow || decision == "allow", Some(id))
        }
        // Write before act: no receipt means DENY when enforcing.
        Err(e) => {
            tracing::warn!(tool = tool.name, "kernel check: receipt not written ({e})");
            done("kernel_unavailable", mode == Mode::Shadow, None)
        }
    }
}

fn enum_name(class: ActionClass) -> &'static str {
    use ActionClass::*;
    match class {
        Read => "read",
        UiLayout => "ui_layout",
        RunControl => "run_control",
        FileWrite => "file_write",
        Network => "network",
        AppInstall => "app_install",
        InboxPrepare => "inbox_prepare",
        Money => "money",
        ClientMessage => "client_message",
        Deploy => "deploy",
        SecretNewHost => "secret_new_host",
        Irreversible => "irreversible",
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

    fn tool<'a>(name: &'a str, args: &'a Value, class: ActionClass, known: bool) -> Tool<'a> {
        Tool { surface: "test", name, args, class, known, approved_by: None }
    }

    #[tokio::test]
    async fn shadow_records_but_proceeds_and_enforce_refuses_a_human_only_class() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        receipts_schema(&state.db).await;
        let a = json!({});
        let inv = tool("send_invoice", &a, ActionClass::Money, true);
        let shadow = check_in(&state.db, Ok(test_key()), Mode::Shadow, "user_1", "chat_1", &inv).await;
        assert_eq!((shadow.decision, shadow.proceed), ("human_only", true));
        assert!(shadow.receipt_id.is_some());
        let enforce = check_in(&state.db, Ok(test_key()), Mode::Enforce, "user_1", "chat_1", &inv).await;
        assert_eq!((enforce.decision, enforce.proceed), ("human_only", false));
        let r = enforce.refusal().unwrap();
        assert_eq!(r["code"], "approval_required");
        assert!(r["action_hash"].as_str().unwrap().starts_with("sha256:"));
        let ws = tool("web_search", &a, ActionClass::Read, true);
        let read = check_in(&state.db, Ok(test_key()), Mode::Enforce, "user_1", "chat_1", &ws).await;
        assert_eq!((read.decision, read.proceed), ("allow", true));
        assert!(read.refusal().is_none());
        // Unknown tool: ask in enforce, never allow.
        let unk = tool("mystery", &a, ActionClass::Read, false);
        let u = check_in(&state.db, Ok(test_key()), Mode::Enforce, "user_1", "chat_1", &unk).await;
        assert_eq!((u.decision, u.proceed), ("ask", false));
        // No key: no receipt, so enforce denies and shadow runs.
        let nokey = check_in(&state.db, Err("unset".into()), Mode::Enforce, "user_1", "chat_1", &ws).await;
        assert_eq!((nokey.decision, nokey.proceed), ("kernel_unavailable", false));
        let nokey = check_in(&state.db, Err("unset".into()), Mode::Shadow, "user_1", "chat_1", &ws).await;
        assert!(nokey.proceed && nokey.receipt_id.is_none());
        // A call a person already approved is recorded and runs.
        let mut ok = tool("send_invoice", &a, ActionClass::Money, true);
        ok.approved_by = Some("meeting_owner");
        assert!(check_in(&state.db, Ok(test_key()), Mode::Enforce, "user_1", "chat_1", &ok).await.proceed);
        // One chain, linked, and the table refuses edits.
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT idx, prev_hash, hash FROM kernel_receipts WHERE principal = 'user_1' ORDER BY idx").fetch_all(&state.db).await.unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].1, GENESIS_HASH);
        assert_eq!(rows[1].1, rows[0].2);
        assert_eq!(rows[2].1, rows[1].2);
        assert!(sqlx::query("UPDATE kernel_receipts SET decision = 'allow'").execute(&state.db).await.is_err());
        assert!(sqlx::query("DELETE FROM kernel_receipts").execute(&state.db).await.is_err());
    }
}
