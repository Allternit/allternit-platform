//! `secret.use`: the kernel attaches credentials at the call (CONTRACT.md 2.3, 3.3; plan step 10).
//!
//! No secret value appears in a request, a response, a receipt or a log. A caller names a handle,
//! a purpose and a target; the kernel resolves the value only inside the tool execution, hands it
//! to the code that attaches it (a header, a process env, a field write) as a [`BoundSecret`]
//! (not `Serialize`, `Debug` prints no value), and adds it to the call's redaction set so every
//! tool result is masked before it reaches the model ([`result_redactor`]).
//!
//! **New hosts.** The first use of a secret toward a host the secret has never been used with is
//! `secret_new_host`, a human-only class: it needs an approval token approved by a person. The pair
//! is written to `kernel_secret_hosts` (migration V300) only after that approval. Later uses toward
//! the same host are allowed and receipted (`kind: secret_use`).
//!
//! | Entry point | Who calls it |
//! |---|---|
//! | [`secret_use_blocking`] | Any host code binding a credential by handle (`SecretUseV1` body + a resolver) |
//! | [`classify_credential_use`] | `check::classify` for computer `use_credential`: known pair = read, new pair = `secret_new_host` |
//! | [`bind_computer_credential`] | `computer_v2::execute_v2` after an ALLOW check: records the pair once the check redeemed a person's token |
//! | [`attach_for_user`] / [`result_redactor`] | Register a value the kernel attached; build the redactor every tool path applies |
//!
//! Modes follow `check` (`ALLTERNIT_KERNEL_CHECK`): `off` binds without receipts, `shadow` binds a
//! new host with a `shadow_would_ask` receipt and does not record the pair, `enforce` refuses a new
//! host without a person's token.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use allternit_authority::policy::Condition;
use allternit_authority::{
    ActionClass, ApprovalRedeemBody, ApprovalRequestV1, AuthorityError, Caller, ErrorCode, ReceiptKind, SecretPurpose,
    SecretUseResult, SecretUseV1, SourceStore,
};
use allternit_factory_engine::receipts::sign::ReceiptSigner;
use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use super::approvals::{SqlAuthority, REQUEST_SCHEMA_ID};
use super::audit::{append, ReceiptInput, DEFAULT_TENANT};
use super::check::{Action, CheckCtx, Classified, Mode, Source};
use super::redact::{looks_like_secret, Redactor};
use super::tokens;

pub const SECRET_USE_SCHEMA_ID: &str = "allternit.os.SecretUseV1";
/// How long an attached value stays in the redaction set after the call that bound it.
const ATTACHED_TTL: StdDuration = StdDuration::from_secs(30 * 60);
/// Minutes a person has to approve a new-host request.
const REQUEST_DEADLINE_MINS: i64 = 30;

// ─── Bound values ───────────────────────────────────────────────────────────────

/// A credential the kernel resolved for one attach. The value is reachable only through
/// [`BoundSecret::expose_for_attach`]; it is not `Serialize`, and `Debug` prints no value.
pub struct BoundSecret {
    value: String,
    pub result: SecretUseResult,
    pub new_host: bool,
}

impl BoundSecret {
    /// The plaintext, for the code that puts it on the wire (a header, a child env, a field write).
    /// Never return it, log it or put it in a tool result.
    pub fn expose_for_attach(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for BoundSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BoundSecret {{ use_id: {}, new_host: {}, value: [REDACTED] }}", self.result.use_id, self.new_host)
    }
}

/// Values the kernel attached recently, per scope (`user:<id>` or `run:<id>`). Memory only.
static ATTACHED: Lazy<Mutex<HashMap<String, Vec<(String, Instant)>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn attach(scope: String, value: &str) {
    if value.is_empty() {
        return;
    }
    let mut map = ATTACHED.lock().unwrap_or_else(|p| p.into_inner());
    let list = map.entry(scope).or_default();
    list.retain(|(_, at)| at.elapsed() < ATTACHED_TTL);
    if !list.iter().any(|(v, _)| v == value) {
        list.push((value.to_string(), Instant::now()));
    }
}

fn attached(scope: &str) -> Vec<String> {
    let mut map = ATTACHED.lock().unwrap_or_else(|p| p.into_inner());
    match map.get_mut(scope) {
        Some(list) => {
            list.retain(|(_, at)| at.elapsed() < ATTACHED_TTL);
            list.iter().map(|(v, _)| v.clone()).collect()
        }
        None => Vec::new(),
    }
}

/// Register a value the kernel attached for this account (computer `use_credential` from the
/// vault, the OS keychain, Bitwarden or 1Password), so tool results are screened for it.
pub fn attach_for_user(user_id: &str, value: &str) {
    attach(format!("user:{user_id}"), value);
}

/// Register a value attached for one run.
pub fn attach_for_run(run_id: &str, value: &str) {
    attach(format!("run:{run_id}"), value);
}

/// This process's own credential-named environment values (provider keys, signing secrets). A
/// shell tool that echoes one gets it masked even when it has no known shape. Read once.
static PROCESS_SECRETS: Lazy<Vec<String>> = Lazy::new(|| {
    std::env::vars()
        .filter(|(k, v)| crate::tool_routes::is_secret_key(k) && v.chars().count() >= 8)
        .map(|(_, v)| v)
        .collect()
});

/// The redactor every tool path applies before a result can reach the model: the account's vault
/// secrets (the same screening set computer use applies), every value the kernel attached for the
/// account or run, this process's credential-named env values, and the credential-shape rules.
pub fn result_redactor(user_id: &str, run_id: Option<&str>) -> Redactor {
    let mut values: Vec<String> =
        crate::aci_credentials::CREDENTIALS.screening_secrets(user_id).into_iter().map(|(_, v, _)| v).collect();
    values.extend(PROCESS_SECRETS.iter().cloned());
    values.extend(attached(&format!("user:{user_id}")));
    if let Some(r) = run_id.filter(|r| !r.is_empty()) {
        values.extend(attached(&format!("run:{r}")));
    }
    Redactor::new(values)
}

// ─── Hosts ──────────────────────────────────────────────────────────────────────

/// `https://API.GitHub.com:443/x` → `api.github.com`. `None` for an empty or malformed host.
pub fn normalize_host(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_ascii_lowercase();
    if let Some((_, rest)) = s.split_once("://") {
        s = rest.to_string();
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h);
    let host = if s.starts_with('[') {
        s.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        s.split(':').next().unwrap_or("").to_string()
    };
    let host = host.trim_end_matches('.').to_string();
    let ok = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':'));
    ok.then_some(host)
}

/// The binding target of a `SecretUseV1`: the host for `http_header` / `connector_call` / `sign`,
/// `process:<name>` for `env_for_process`.
fn target_of(body: &SecretUseV1) -> Result<String, AuthorityError> {
    let host = body.target_host.clone().flatten();
    let process = body.target_process.clone().flatten();
    match body.purpose {
        SecretPurpose::EnvForProcess => process
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .map(|p| format!("process:{p}"))
            .ok_or_else(|| bad("target_process is required for env_for_process")),
        SecretPurpose::HttpHeader | SecretPurpose::ConnectorCall => host
            .as_deref()
            .and_then(normalize_host)
            .ok_or_else(|| bad("target_host is required for http_header and connector_call")),
        SecretPurpose::Sign => Ok(host.as_deref().and_then(normalize_host).unwrap_or_else(|| "sign:local".into())),
    }
}

fn bad(msg: impl Into<String>) -> AuthorityError {
    AuthorityError::without_receipt(ErrorCode::BadRequest, msg)
}

fn store(e: impl std::fmt::Display) -> AuthorityError {
    AuthorityError::kernel_unavailable(format!("kernel store: {e}"))
}

/// A handle names a secret; it is never the secret.
fn check_handle(secret_ref: &str) -> Result<(), AuthorityError> {
    let r = secret_ref.trim();
    if r.is_empty() || r.len() > 200 || r.chars().any(char::is_whitespace) || looks_like_secret(r) {
        return Err(bad("secret_ref must be a handle, never a value"));
    }
    Ok(())
}

fn known(conn: &Connection, owner: &str, tenant: &str, secret_ref: &str, host: &str) -> Result<bool, AuthorityError> {
    conn.query_row(
        "SELECT 1 FROM kernel_secret_hosts WHERE owner_user_id = ?1 AND tenant = ?2 AND secret_ref = ?3 AND host = ?4",
        params![owner, tenant, secret_ref, host],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
    .map_err(store)
}

fn touch(conn: &Connection, owner: &str, tenant: &str, secret_ref: &str, host: &str, now: DateTime<Utc>) -> Result<(), AuthorityError> {
    conn.execute(
        "UPDATE kernel_secret_hosts SET use_count = use_count + 1, last_used_at = ?5
         WHERE owner_user_id = ?1 AND tenant = ?2 AND secret_ref = ?3 AND host = ?4",
        params![owner, tenant, secret_ref, host, tokens::ts(now)],
    )
    .map(|_| ())
    .map_err(store)
}

#[allow(clippy::too_many_arguments)]
fn record(
    conn: &Connection,
    owner: &str,
    tenant: &str,
    secret_ref: &str,
    host: &str,
    use_id: &str,
    approval_receipt: &str,
    now: DateTime<Utc>,
) -> Result<(), AuthorityError> {
    conn.execute(
        "INSERT INTO kernel_secret_hosts (owner_user_id, tenant, secret_ref, host, first_use_id, approval_receipt, first_used_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT (owner_user_id, tenant, secret_ref, host) DO UPDATE SET use_count = use_count + 1, last_used_at = excluded.last_used_at",
        params![owner, tenant, secret_ref, host, use_id, approval_receipt, tokens::ts(now)],
    )
    .map(|_| ())
    .map_err(store)
}

struct Receipts<'a> {
    ctx: &'a CheckCtx,
    signer: &'a Arc<ReceiptSigner>,
    run_id: String,
    tenant: String,
    now: DateTime<Utc>,
}

impl Receipts<'_> {
    /// Write before act: the receipt for a bind goes in before the value is handed over.
    fn write(&self, decision: &str, mut payload: Value) -> Result<String, AuthorityError> {
        payload["decision"] = json!(decision);
        let mut conn = self.ctx.db.connect().map_err(store)?;
        append(
            &mut conn,
            self.signer,
            ReceiptInput {
                run_id: &self.run_id,
                principal: &self.ctx.user_id,
                tenant: &self.tenant,
                kind: ReceiptKind::SecretUse,
                schema_id: SECRET_USE_SCHEMA_ID,
                decision,
                payload,
                idempotency_key: None,
            },
            self.now,
        )
    }
}

fn purpose_str(p: SecretPurpose) -> &'static str {
    match p {
        SecretPurpose::HttpHeader => "http_header",
        SecretPurpose::EnvForProcess => "env_for_process",
        SecretPurpose::ConnectorCall => "connector_call",
        SecretPurpose::Sign => "sign",
    }
}

/// The canonical action a new-host approval is bound to: `{tool: "secret.use", args: {secret_ref, target, purpose}}`.
pub fn use_payload(secret_ref: &str, target: &str, purpose: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("tool".into(), json!("secret.use"));
    m.insert("args".into(), json!({ "secret_ref": secret_ref, "target": target, "purpose": purpose }));
    m
}

/// `secret.use(handle, host, purpose)`. `resolve` turns the handle into the value and is called
/// only after the bind is allowed and receipted. Errors: `bad_request` (a value where a handle
/// belongs, a missing target), `approval_required` (new host, no token; an approval request is
/// opened), the redeem errors for a bad token, `not_found` (unknown handle), `kernel_unavailable`.
pub fn secret_use_blocking(
    ctx: &CheckCtx,
    signer: Arc<ReceiptSigner>,
    body: &SecretUseV1,
    resolve: impl FnOnce(&str) -> Option<String>,
    now: DateTime<Utc>,
) -> Result<BoundSecret, AuthorityError> {
    if body.schema_id != SECRET_USE_SCHEMA_ID {
        return Err(bad(format!("schema_id must be {SECRET_USE_SCHEMA_ID}")));
    }
    check_handle(&body.secret_ref)?;
    if !tokens::is_content_hash(&body.action_hash) {
        return Err(bad("action_hash must be sha256:<64 hex>"));
    }
    let target = target_of(body)?;
    let purpose = purpose_str(body.purpose);
    let mode = ctx.mode();
    let tenant = ctx.tenant.clone().unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let run_id = ctx.run_id.clone().unwrap_or_else(|| "tool:secret_use".into());
    let use_hash = tokens::action_hash(&use_payload(&body.secret_ref, &target, purpose))
        .map_err(|e| AuthorityError::kernel_unavailable(format!("secret.use payload: {e}")))?;
    let rec = Receipts { ctx, signer: &signer, run_id: run_id.clone(), tenant: tenant.clone(), now };
    let redaction_id = format!("red_{}", uuid::Uuid::new_v4().simple());
    let mut receipt = json!({
        "event": "secret_use", "use_id": body.use_id, "secret_ref": body.secret_ref, "target": target,
        "purpose": purpose, "action_hash": body.action_hash, "use_hash": use_hash, "check_id": body.check_id,
        "redaction_id": redaction_id, "mode": mode.as_str(),
    });

    let bound = |new_host: bool, receipt_id: String| -> Result<BoundSecret, AuthorityError> {
        let value = resolve(&body.secret_ref)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| AuthorityError::new(ErrorCode::NotFound, "no secret with that handle", Some(receipt_id.clone())))?;
        attach(format!("user:{}", ctx.user_id), &value);
        attach(format!("run:{run_id}"), &value);
        Ok(BoundSecret {
            value,
            new_host,
            result: SecretUseResult { use_id: body.use_id.clone(), bound: true, redaction_id: redaction_id.clone(), receipt_id },
        })
    };

    if mode == Mode::Off {
        return bound(false, String::new());
    }
    let conn = ctx.db.connect().map_err(store)?;
    let is_known = known(&conn, &ctx.user_id, &tenant, &body.secret_ref, &target)?;
    receipt["new_host"] = json!(!is_known);
    if is_known {
        touch(&conn, &ctx.user_id, &tenant, &body.secret_ref, &target, now)?;
        let rid = rec.write("allow", receipt)?;
        return bound(false, rid);
    }

    // New host: human-only. A token approved by a person for this exact use, or nothing binds.
    let caller = Caller { kind: ctx.caller_kind, principal: ctx.user_id.clone() };
    let kernel = SqlAuthority::new(ctx.db.clone(), signer.clone()).with_tenant(ctx.tenant.clone()).at(now);
    let token = body.approval_token.clone().flatten().filter(|t| !t.trim().is_empty());
    if let Some(token) = token {
        return match kernel.redeem(&caller, ApprovalRedeemBody { token, action_hash: use_hash.clone() }) {
            Ok(r) => {
                receipt["approval_request_id"] = json!(r.request_id);
                receipt["spend_receipt_id"] = json!(r.spend_receipt_id);
                let rid = rec.write("allow", receipt)?;
                record(&conn, &ctx.user_id, &tenant, &body.secret_ref, &target, &body.use_id, &rid, now)?;
                bound(true, rid)
            }
            Err(e) if mode == Mode::Enforce => Err(e),
            Err(e) => {
                receipt["would_deny"] = json!(format!("{:?}", e.code));
                let rid = rec.write("shadow_deny", receipt)?;
                bound(true, rid)
            }
        };
    }
    if mode == Mode::Shadow {
        receipt["would_require"] = json!("human_approval");
        let rid = rec.write("shadow_would_ask", receipt)?;
        return bound(true, rid);
    }
    let summary: String = format!("Use secret {} toward {} for the first time", body.secret_ref, target).chars().take(280).collect();
    let request = ApprovalRequestV1 {
        schema_id: REQUEST_SCHEMA_ID.into(),
        schema_version: tokens::SCHEMA_VERSION.into(),
        request_id: None,
        source_store: SourceStore::Kernel,
        source_ref: Some(body.use_id.clone()),
        run_id: run_id.clone(),
        principal: format!("run:{run_id}"),
        assignee: "owner".into(),
        action_class: ActionClass::SecretNewHost,
        action_summary: summary,
        action_payload: use_payload(&body.secret_ref, &target, purpose),
        action_hash: None,
        deadline: tokens::ts(now + Duration::minutes(REQUEST_DEADLINE_MINS)),
        on_expiry: "reject".into(),
        attention_id: None,
        check_id: body.check_id.clone(),
        status: None,
        created_at: None,
        resolved_at: None,
        resolved_by: None,
        evidence_refs: None,
        extensions: None,
    };
    let request_id = kernel.request(&caller, request).ok().and_then(|r| r.request_id);
    receipt["approval_request_id"] = json!(request_id);
    let rid = rec.write("approval_required", receipt)?;
    Err(AuthorityError::new(
        ErrorCode::ApprovalRequired,
        match &request_id {
            Some(id) => format!("First use of this secret toward {target}: a person must approve request {id}, then retry with approval_token."),
            None => format!("First use of this secret toward {target}: a person must approve it first."),
        },
        Some(rid),
    ))
}

// ─── Computer `use_credential` ──────────────────────────────────────────────────

/// `computer:<source>:<name>`: the handle of a computer credential (vault, keychain, bitwarden,
/// onepassword). The value never leaves the computer toolset.
fn computer_handle(args: &Value) -> Option<String> {
    let name = args.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty())?;
    let source = args.get("source").and_then(Value::as_str).unwrap_or("default");
    Some(format!("computer:{source}:{name}"))
}

/// Where a computer credential is written: the declared `domain`, else the computer itself.
fn computer_target(args: &Value) -> String {
    args.get("domain")
        .and_then(Value::as_str)
        .and_then(normalize_host)
        .unwrap_or_else(|| format!("computer:{}", args.get("computer_id").and_then(Value::as_str).unwrap_or("unknown")))
}

/// `check::classify` for computer `use_credential`: a (credential, target) pair already approved
/// for this account is `read` (allowed, receipted); a new pair is `secret_new_host` (human-only).
/// A store error answers `secret_new_host` (the strict side).
pub fn classify_credential_use(ctx: &CheckCtx, args: &Value) -> Classified {
    let class = (|| {
        let handle = computer_handle(args)?;
        let conn = ctx.db.connect().ok()?;
        let tenant = ctx.tenant.clone().unwrap_or_else(|| DEFAULT_TENANT.to_string());
        known(&conn, &ctx.user_id, &tenant, &handle, &computer_target(args)).ok()?.then_some(ActionClass::Read)
    })();
    match class {
        Some(c) => Classified { class: c, cond: Condition::NONE, by: "secret_known_host", forced_ask: false },
        None => Classified { class: ActionClass::SecretNewHost, cond: Condition::NONE, by: "secret_new_host", forced_ask: false },
    }
}

/// After `check` allowed a computer `use_credential` (`args` = the checked arguments, with
/// `computer_id`): a known pair gets a `secret_use` receipt; a new pair is recorded only when the
/// check receipt shows a person's token was redeemed for this exact action. Shadow mode receipts
/// and records nothing new; enforce refuses a new pair without that token.
pub fn bind_computer_credential_blocking(
    ctx: &CheckCtx,
    signer: Arc<ReceiptSigner>,
    args: &Value,
    check_receipt_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), AuthorityError> {
    let mode = ctx.mode();
    if mode == Mode::Off {
        return Ok(());
    }
    let handle = computer_handle(args).ok_or_else(|| bad("use_credential needs the credential's name"))?;
    let target = computer_target(args);
    let tenant = ctx.tenant.clone().unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let run_id = ctx.run_id.clone().unwrap_or_else(|| "tool:computer".into());
    let action_hash = tokens::action_hash(&Action::new(Source::Computer, "use_credential", args).payload())
        .map_err(|e| AuthorityError::kernel_unavailable(format!("use_credential payload: {e}")))?;
    let rec = Receipts { ctx, signer: &signer, run_id, tenant: tenant.clone(), now };
    let use_id = format!("use_{}", uuid::Uuid::new_v4().simple());
    let mut receipt = json!({
        "event": "secret_use", "use_id": use_id, "secret_ref": handle, "target": target, "purpose": "computer_field",
        "action_hash": action_hash, "check_receipt_id": check_receipt_id, "mode": mode.as_str(),
    });
    let conn = ctx.db.connect().map_err(store)?;
    if known(&conn, &ctx.user_id, &tenant, &handle, &target)? {
        touch(&conn, &ctx.user_id, &tenant, &handle, &target, now)?;
        receipt["new_host"] = json!(false);
        rec.write("allow", receipt)?;
        return Ok(());
    }
    receipt["new_host"] = json!(true);
    // Did the check that let this call through redeem a person's token for this exact action?
    let approved = check_receipt_id
        .map(|rid| {
            conn.query_row(
                "SELECT decision, principal, json_extract(payload, '$.action_class'), json_extract(payload, '$.action_hash'),
                        json_extract(payload, '$.spend_receipt_id')
                 FROM kernel_receipts WHERE receipt_id = ?1 AND kind = 'check'",
                params![rid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)),
            )
            .optional()
        })
        .transpose()
        .map_err(store)?
        .flatten()
        .is_some_and(|(decision, principal, class, hash, spend)| {
            decision == "allow"
                && principal == ctx.user_id
                && class.as_deref() == Some("secret_new_host")
                && hash.as_deref() == Some(action_hash.as_str())
                && spend.is_some()
        });
    if approved {
        let rid = rec.write("allow", receipt)?;
        record(&conn, &ctx.user_id, &tenant, &handle, &target, &use_id, &rid, now)?;
        return Ok(());
    }
    if mode == Mode::Shadow {
        receipt["would_require"] = json!("human_approval");
        rec.write("shadow_would_ask", receipt)?;
        return Ok(());
    }
    let rid = rec.write("denied", receipt)?;
    Err(AuthorityError::new(
        ErrorCode::ApprovalRequired,
        format!("First use of credential toward {target}: a person must approve it (retry the same call with kernel_approval_token)."),
        Some(rid),
    ))
}

/// Async wrapper for [`bind_computer_credential_blocking`]: the refusal as a JSON string (same
/// shape as a `check` refusal), `Ok` when the credential may be written.
pub async fn bind_computer_credential(ctx: &CheckCtx, args: &Value, check_receipt_id: Option<String>) -> Result<(), String> {
    let mode = ctx.mode();
    if mode == Mode::Off {
        return Ok(());
    }
    let signer = match tokens::kernel_signer(&ctx.data_dir()) {
        Ok(s) => s,
        Err(e) if mode == Mode::Enforce => return Err(refusal(&e)),
        Err(e) => {
            tracing::warn!(error = %e.message, "secret.use unavailable (shadow mode: credential binds)");
            return Ok(());
        }
    };
    let (ctx, args) = (ctx.clone(), args.clone());
    let out = tokio::task::spawn_blocking(move || bind_computer_credential_blocking(&ctx, signer, &args, check_receipt_id.as_deref(), Utc::now()))
        .await
        .unwrap_or_else(|e| Err(AuthorityError::kernel_unavailable(format!("secret.use task: {e}"))));
    match out {
        Ok(()) => Ok(()),
        Err(e) if mode == Mode::Enforce => Err(refusal(&e)),
        Err(e) => {
            tracing::warn!(error = %e.message, "secret.use failed (shadow mode: credential binds)");
            Ok(())
        }
    }
}

fn refusal(e: &AuthorityError) -> String {
    let code = serde_json::to_value(e.code).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    json!({ "code": code, "error": code, "message": e.message, "receipt_id": e.receipt_id }).to_string()
}

#[cfg(test)]
mod tests {
    use super::super::approvals::{Decision as Resolve, Resolution, ResolveBody};
    use super::super::check::{check_blocking, Decision};
    use super::*;
    use crate::db::DbHandle;
    use allternit_authority::CallerKind;

    const USER: &str = "user_owner";
    const VALUE: &str = "vault-plain-value-0042";

    fn t0() -> DateTime<Utc> {
        tokens::parse_ts("2026-11-28T12:00:00Z").unwrap()
    }
    fn signer() -> Arc<ReceiptSigner> {
        Arc::new(ReceiptSigner::from_seed([7u8; 32]))
    }
    fn ctx(db: &DbHandle) -> CheckCtx {
        CheckCtx::new(db.clone(), USER, None).with_run_id(Some("run_s".into())).with_mode(Mode::Enforce)
    }
    fn body(host: &str, token: Option<String>) -> SecretUseV1 {
        SecretUseV1 {
            schema_id: SECRET_USE_SCHEMA_ID.into(),
            schema_version: tokens::SCHEMA_VERSION.into(),
            use_id: format!("use_{}", uuid::Uuid::new_v4().simple()),
            secret_ref: "vault:github-deploy".into(),
            purpose: SecretPurpose::HttpHeader,
            target_host: Some(Some(host.into())),
            target_process: None,
            action_hash: format!("sha256:{}", "a".repeat(64)),
            check_id: "chk_test".into(),
            approval_token: Some(token),
            new_host: None,
            redaction_id: None,
            receipt_id: None,
            extensions: None,
        }
    }
    fn approve(db: &DbHandle, request_id: &str) -> String {
        let k = SqlAuthority::new(db.clone(), signer()).at(t0());
        let human = Caller { kind: CallerKind::Human, principal: USER.into() };
        match k.resolve(&human, ResolveBody { request_id: request_id.into(), decision: Resolve::Approve, note: None }).unwrap() {
            Resolution::Approved(t) => t.token,
            _ => panic!("expected a token"),
        }
    }
    fn open_request(db: &DbHandle) -> String {
        db.connect()
            .unwrap()
            .query_row("SELECT request_id FROM approval_requests WHERE status = 'open' ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn first_use_to_a_new_host_needs_a_person_and_the_second_use_passes() {
        let db = DbHandle::new_memory().unwrap();
        let c = ctx(&db);
        // First use toward api.github.com, no token: refused, nothing resolved, a request opened.
        let mut resolved = false;
        let e = secret_use_blocking(&c, signer(), &body("https://API.github.com/repos", None), |_| { resolved = true; Some(VALUE.into()) }, t0()).unwrap_err();
        assert_eq!(e.code, ErrorCode::ApprovalRequired);
        assert!(e.receipt_id.is_some());
        assert!(!resolved, "the value is never resolved before the bind is allowed");
        assert!(!e.message.contains(VALUE));

        // A person approves; the retry with the token binds and records the pair.
        let token = approve(&db, &open_request(&db));
        let b = secret_use_blocking(&c, signer(), &body("api.github.com", Some(token.clone())), |_| Some(VALUE.into()), t0()).unwrap();
        assert!(b.new_host && b.result.bound);
        assert_eq!(b.expose_for_attach(), VALUE);
        assert!(!format!("{b:?}").contains(VALUE));
        assert!(!serde_json::to_string(&b.result).unwrap().contains(VALUE));

        // Second use to the same host: no token, allowed and receipted.
        let b2 = secret_use_blocking(&c, signer(), &body("api.github.com:443", None), |_| Some(VALUE.into()), t0()).unwrap();
        assert!(!b2.new_host && !b2.result.receipt_id.is_empty());

        // Another host is new again; the spent token does not carry over.
        let e = secret_use_blocking(&c, signer(), &body("evil.example.com", Some(token)), |_| Some(VALUE.into()), t0()).unwrap_err();
        assert_ne!(e.code, ErrorCode::BadRequest);

        // No receipt holds the value.
        let payloads: Vec<String> = db
            .connect()
            .unwrap()
            .prepare("SELECT payload FROM kernel_receipts")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(payloads.iter().all(|p| !p.contains(VALUE)));
        assert!(payloads.iter().any(|p| p.contains("\"secret_use\"")));
    }

    #[test]
    fn a_value_is_refused_where_a_handle_belongs() {
        let db = DbHandle::new_memory().unwrap();
        let mut b = body("api.github.com", None);
        b.secret_ref = format!("{}{}", "ghp", "_TestOnlyNotARealToken000000000000abcd");
        let e = secret_use_blocking(&ctx(&db), signer(), &b, |_| None, t0()).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
    }

    #[test]
    fn bound_values_are_masked_in_tool_results() {
        let db = DbHandle::new_memory().unwrap();
        let c = ctx(&db).with_mode(Mode::Shadow);
        let b = secret_use_blocking(&c, signer(), &body("api.example.com", None), |_| Some(VALUE.into()), t0()).unwrap();
        let out = result_redactor(USER, Some("run_s")).text(&format!("curl said {} ok", b.expose_for_attach()));
        assert!(!out.contains(VALUE), "{out}");
    }

    #[test]
    fn computer_credential_new_target_needs_a_token_then_is_known() {
        let db = DbHandle::new_memory().unwrap();
        let c = ctx(&db);
        let args = json!({ "name": "github-login", "domain": "github.com", "element_id": "e4", "computer_id": "cmp_1" });
        let action = Action::new(Source::Computer, "use_credential", &args);

        // New pair: check asks for a person (secret_new_host).
        let Decision::NeedsToken { request_id, action_class, .. } = check_blocking(&c, &action, Mode::Enforce, signer(), t0()) else {
            panic!("expected approval_required")
        };
        assert_eq!(action_class, ActionClass::SecretNewHost);
        assert!(bind_computer_credential_blocking(&c, signer(), &args, None, t0()).is_err());

        // Approved: the check redeems the token, the bind records the pair.
        let token = approve(&db, &request_id.unwrap());
        let Decision::Allow { receipt_id } = check_blocking(&c.clone().with_token(Some(token)), &action, Mode::Enforce, signer(), t0()) else {
            panic!("expected allow")
        };
        bind_computer_credential_blocking(&c, signer(), &args, receipt_id.as_deref(), t0()).unwrap();

        // Same credential, same domain: allowed without a token.
        assert_eq!(classify_credential_use(&c, &args).class, ActionClass::Read);
        assert!(check_blocking(&c, &action, Mode::Enforce, signer(), t0()).allowed());
        bind_computer_credential_blocking(&c, signer(), &args, None, t0()).unwrap();

        // Another domain is new again.
        let other = json!({ "name": "github-login", "domain": "gitlab.com", "element_id": "e4", "computer_id": "cmp_1" });
        assert_eq!(classify_credential_use(&c, &other).class, ActionClass::SecretNewHost);
    }

    #[test]
    fn hosts_normalise() {
        assert_eq!(normalize_host("https://API.GitHub.com:443/x?y").as_deref(), Some("api.github.com"));
        assert_eq!(normalize_host("user:pw@example.com.").as_deref(), Some("example.com"));
        assert_eq!(normalize_host("  "), None);
    }
}
