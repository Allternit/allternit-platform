//! `approval.request`, `approval.resolve` and `approval.redeem` over SQLite (CONTRACT.md 2.4, 3.1, 4.3).
//!
//! Every outcome, allowed or denied, writes a `kernel_receipts` row first, in the same
//! transaction as the state change (write before act, CONTRACT.md 5.1). A denial carries its
//! `receipt_id`; only `bad_request` does not.

use std::sync::Arc;

use allternit_authority::{
    ActionClass, ApprovalRedeemBody, ApprovalRedeemResult, ApprovalRequestV1, ApprovalStatus, ApprovalTokenV1, AuditAppendBody,
    Authority, AuthorityError, BudgetCommitBody, BudgetReleaseBody, BudgetReservationV1, Caller, CallerKind, DeployAdmissionV1,
    ErrorCode, LeaseRequest, LeaseResponse, PolicyCheck, PolicyDecision, ReceiptChain, ReceiptKind, SecretUseResult, SecretUseV1,
    SourceStore, OS_EXTENSION_KEY,
};
use allternit_factory_engine::receipts::jcs;
use allternit_factory_engine::receipts::sign::ReceiptSigner;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::tokens::{self, TokenClaims};
use super::audit::{append, append_in_tx, ReceiptInput};
use crate::db::DbHandle;

pub const REQUEST_SCHEMA_ID: &str = "allternit.os.ApprovalRequestV1";
const MAX_SUMMARY_CHARS: usize = 280;

/// `approval.resolve` body (CONTRACT.md 2.4). Not part of the crate's trait on purpose.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveBody {
    pub request_id: String,
    pub decision: Decision,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Deny,
}

/// Approve returns the token; deny returns the request with status `denied`.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum Resolution {
    Approved(ApprovalTokenV1),
    Denied(ApprovalRequestV1),
}

/// The kernel's approval store. `caller.principal` is the authenticated account (set by the
/// transport); requests are scoped to it.
pub struct SqlAuthority {
    db: DbHandle,
    signer: Arc<ReceiptSigner>,
    now: Option<DateTime<Utc>>,
    /// Chain scope with the principal (CONTRACT.md 11.2).
    tenant: String,
}

struct Stored {
    request: ApprovalRequestV1,
    owner_user_id: String,
    token_ttl_secs: u64,
    token_id: Option<String>,
}

fn enum_str<T: Serialize>(v: T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn enum_from<T: for<'de> Deserialize<'de>>(s: &str) -> Option<T> {
    serde_json::from_value(Value::String(s.to_string())).ok()
}

fn bad(msg: impl Into<String>) -> AuthorityError {
    AuthorityError::without_receipt(ErrorCode::BadRequest, msg)
}

fn db_err(e: rusqlite::Error) -> AuthorityError {
    AuthorityError::kernel_unavailable(format!("kernel store: {e}"))
}

/// `extensions["x-os-v1"].token_ttl_secs`, the TTL the requester asks for.
fn requested_ttl(ext: Option<&Map<String, Value>>) -> Option<u64> {
    ext?.get(OS_EXTENSION_KEY)?.get("token_ttl_secs")?.as_u64()
}

impl SqlAuthority {
    pub fn new(db: DbHandle, signer: Arc<ReceiptSigner>) -> Self {
        Self { db, signer, now: None, tenant: super::audit::DEFAULT_TENANT.to_string() }
    }

    /// Chain scope: the caller's tenant (`AuthUser::tenant_id`); `default` when there is none.
    pub fn with_tenant(mut self, tenant: Option<String>) -> Self {
        self.tenant = tenant.filter(|t| !t.is_empty()).unwrap_or_else(|| super::audit::DEFAULT_TENANT.to_string());
        self
    }

    fn input<'a>(&'a self, run_id: &'a str, principal: &'a str, decision: &'a str, payload: Value) -> ReceiptInput<'a> {
        ReceiptInput {
            run_id,
            principal,
            tenant: &self.tenant,
            kind: ReceiptKind::Approval,
            schema_id: REQUEST_SCHEMA_ID,
            decision,
            payload,
            idempotency_key: None,
        }
    }

    /// Fix the clock (tests).
    pub fn at(mut self, now: DateTime<Utc>) -> Self {
        self.now = Some(now);
        self
    }

    fn now(&self) -> DateTime<Utc> {
        self.now.unwrap_or_else(Utc::now)
    }

    fn connect(&self) -> Result<Connection, AuthorityError> {
        self.db.connect().map_err(db_err)
    }

    /// Write a denial receipt in its own transaction and return the error carrying it.
    fn deny(&self, principal: &str, chain: &str, code: ErrorCode, message: &str, mut payload: Value) -> AuthorityError {
        payload["event"] = json!("denied");
        payload["code"] = json!(enum_str(code));
        payload["reason"] = json!(message);
        let written = self.connect().and_then(|mut conn| {
            append(&mut conn, &self.signer, self.input(chain, principal, "denied", payload), self.now())
        });
        match written {
            Ok(receipt_id) => AuthorityError::new(code, message, Some(receipt_id)),
            // No receipt, no answer that could let anything run: fail closed.
            Err(e) => e,
        }
    }

    fn load(&self, conn: &Connection, request_id: &str) -> Result<Option<Stored>, AuthorityError> {
        conn.query_row(
            "SELECT request_id, owner_user_id, source_store, source_ref, run_id, principal, assignee, action_class,
                    action_summary, action_payload, action_hash, deadline, on_expiry, attention_id, check_id, status,
                    token_ttl_secs, token_id, created_at, resolved_at, resolved_by, evidence_refs, extensions
             FROM approval_requests WHERE request_id = ?1",
            params![request_id],
            |r| {
                let payload: String = r.get(9)?;
                let evidence: Option<String> = r.get(21)?;
                let ext: Option<String> = r.get(22)?;
                let source: String = r.get(2)?;
                let class: String = r.get(7)?;
                let status: String = r.get(15)?;
                Ok((
                    ApprovalRequestV1 {
                        schema_id: REQUEST_SCHEMA_ID.into(),
                        schema_version: tokens::SCHEMA_VERSION.into(),
                        request_id: Some(r.get(0)?),
                        source_store: enum_from(&source).unwrap_or(SourceStore::Kernel),
                        source_ref: r.get(3)?,
                        run_id: r.get(4)?,
                        principal: r.get(5)?,
                        assignee: r.get(6)?,
                        // An unreadable class is treated as the strictest one, never as allow.
                        action_class: enum_from(&class).unwrap_or(ActionClass::Irreversible),
                        action_summary: r.get(8)?,
                        action_payload: serde_json::from_str(&payload).unwrap_or_default(),
                        action_hash: Some(r.get(10)?),
                        deadline: r.get(11)?,
                        on_expiry: r.get(12)?,
                        attention_id: Some(r.get(13)?),
                        check_id: r.get(14)?,
                        status: enum_from(&status),
                        created_at: Some(r.get(18)?),
                        resolved_at: r.get(19)?,
                        resolved_by: r.get(20)?,
                        evidence_refs: evidence.and_then(|s| serde_json::from_str(&s).ok()),
                        extensions: ext.and_then(|s| serde_json::from_str(&s).ok()),
                    },
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(16)?,
                    r.get::<_, Option<String>>(17)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)
        .map(|row| {
            row.map(|(request, owner_user_id, ttl, token_id)| Stored {
                request,
                owner_user_id,
                token_ttl_secs: ttl.clamp(1, tokens::MAX_TTL_SECS as i64) as u64,
                token_id,
            })
        })
    }

    /// `approval.request`: validate, compute the payload hash, write the receipt, store the row.
    pub fn request(&self, caller: &Caller, mut body: ApprovalRequestV1) -> Result<ApprovalRequestV1, AuthorityError> {
        let now = self.now();
        if body.schema_id != REQUEST_SCHEMA_ID {
            return Err(bad(format!("schema_id must be {REQUEST_SCHEMA_ID}")));
        }
        if body.on_expiry != "reject" {
            return Err(bad("on_expiry must be \"reject\": a request never auto-approves"));
        }
        let summary_len = body.action_summary.chars().count();
        if summary_len == 0 || summary_len > MAX_SUMMARY_CHARS {
            return Err(bad("action_summary must be 1-280 characters"));
        }
        for (name, v) in [("run_id", &body.run_id), ("principal", &body.principal), ("assignee", &body.assignee), ("check_id", &body.check_id)] {
            if v.trim().is_empty() {
                return Err(bad(format!("{name} is required")));
            }
        }
        let deadline = tokens::parse_ts(&body.deadline).ok_or_else(|| bad("deadline must be an RFC 3339 timestamp"))?;
        if deadline <= now {
            return Err(bad("deadline is in the past"));
        }
        let hash = tokens::action_hash(&body.action_payload).map_err(|e| bad(format!("action_payload is not canonicalisable: {e}")))?;
        if body.action_hash.as_deref().is_some_and(|h| h != hash) {
            return Err(bad("action_hash does not match the canonical hash of action_payload"));
        }
        let payload_text = jcs::canonicalize(&Value::Object(body.action_payload.clone())).map_err(|e| bad(e.to_string()))?;
        let ttl = tokens::clamp_ttl(requested_ttl(body.extensions.as_ref()));

        // Server-set fields (CONTRACT.md 3.1): whatever the client sent is replaced.
        let request_id = format!("apr_{}", uuid::Uuid::new_v4().simple());
        let attention_id = format!("att_{}", uuid::Uuid::new_v4().simple());
        body.request_id = Some(request_id.clone());
        body.attention_id = Some(attention_id.clone());
        body.action_hash = Some(hash.clone());
        body.status = Some(ApprovalStatus::Open);
        body.created_at = Some(tokens::ts(now));
        body.resolved_at = None;
        body.resolved_by = None;

        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        append_in_tx(
            &tx,
            &self.signer,
            self.input(
                &body.run_id,
                &caller.principal,
                "request",
                json!({
                    "event": "request", "request_id": request_id, "action_class": enum_str(body.action_class),
                    "action_hash": hash, "principal": body.principal, "owner": caller.principal,
                    "caller_kind": enum_str(caller.kind), "deadline": body.deadline, "token_ttl_secs": ttl,
                }),
            ),
            now,
        )?;
        tx.execute(
            "INSERT INTO approval_requests (request_id, owner_user_id, caller_kind, source_store, source_ref, run_id, principal,
                 assignee, action_class, action_summary, action_payload, action_hash, deadline, on_expiry, attention_id,
                 check_id, status, token_ttl_secs, created_at, evidence_refs, extensions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'reject', ?14, ?15, 'open', ?16, ?17, ?18, ?19)",
            params![
                request_id,
                caller.principal,
                enum_str(caller.kind),
                enum_str(body.source_store),
                body.source_ref,
                body.run_id,
                body.principal,
                body.assignee,
                enum_str(body.action_class),
                body.action_summary,
                payload_text,
                hash,
                body.deadline,
                attention_id,
                body.check_id,
                ttl as i64,
                tokens::ts(now),
                body.evidence_refs.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
                body.extensions.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(body)
    }

    /// Refuse an approve that came through the Desktop window without a user-presence proof
    /// (CONTRACT.md 12.3; see [`super::presence`]). Writes the refusal receipt on the request's
    /// chain, like every other resolve refusal, and changes nothing else.
    pub fn refuse_without_presence(&self, caller: &Caller, body: &ResolveBody) -> AuthorityError {
        let chain = self
            .connect()
            .and_then(|conn| self.load(&conn, &body.request_id))
            .ok()
            .flatten()
            .filter(|s| s.owner_user_id == caller.principal)
            .map_or_else(|| format!("kernel:{}", caller.principal), |s| s.request.run_id);
        let ctx = json!({
            "op": "resolve", "request_id": body.request_id, "decision": enum_str(body.decision),
            "caller_kind": enum_str(caller.kind), "caller": caller.principal,
        });
        self.deny(
            &caller.principal,
            &chain,
            ErrorCode::ForbiddenCaller,
            "approving on Allternit Desktop needs a user-presence check (Touch ID or Windows Hello); approve from the Allternit web or phone app if this computer has neither",
            ctx,
        )
    }

    /// `approval.resolve`: HUMAN ONLY (CONTRACT.md 4.3). Any other caller kind is refused with
    /// `forbidden_caller` and a receipt, before the request is even looked at for state.
    pub fn resolve(&self, caller: &Caller, body: ResolveBody) -> Result<Resolution, AuthorityError> {
        let now = self.now();
        let conn = self.connect()?;
        let stored = self.load(&conn, &body.request_id)?;
        drop(conn);
        let fallback_chain = format!("kernel:{}", caller.principal);
        let chain = stored.as_ref().map_or(fallback_chain.as_str(), |s| s.request.run_id.as_str()).to_string();
        let ctx = json!({
            "op": "resolve", "request_id": body.request_id, "decision": enum_str(body.decision),
            "caller_kind": enum_str(caller.kind), "caller": caller.principal,
        });

        if caller.kind != CallerKind::Human {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::ForbiddenCaller, "only a person's session can resolve an approval", ctx));
        }
        // Another account's request reads as not found, so ids do not leak across accounts.
        let Some(stored) = stored.filter(|s| s.owner_user_id == caller.principal) else {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::NotFound, "no such approval request", ctx));
        };
        let req = &stored.request;
        if req.assignee != "owner" && req.assignee != caller.principal {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::ForbiddenCaller, "this request is assigned to someone else", ctx));
        }
        // No self-approval through a thin human wrapper: the resolver is not the acting principal.
        if req.principal == caller.principal {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::ForbiddenCaller, "the requesting principal cannot resolve its own request", ctx));
        }
        if req.status != Some(ApprovalStatus::Open) {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::Conflict, "request is already resolved", ctx));
        }
        let expired = tokens::parse_ts(&req.deadline).map_or(true, |d| now > d);

        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        let resolved_at = tokens::ts(now);
        let mut payload = ctx.clone();

        if expired {
            payload["event"] = json!("expired");
            let rid = append_in_tx(&tx, &self.signer, self.input(&chain, &caller.principal, "expired", payload), now)?;
            if open_guard(&tx, "UPDATE approval_requests SET status = 'expired', resolved_at = ?2 WHERE request_id = ?1 AND status = 'open'", &[&body.request_id, &resolved_at]).is_err() {
                drop(tx);
                return Err(self.deny(&caller.principal, &chain, ErrorCode::Conflict, "request changed while resolving", ctx));
            }
            tx.commit().map_err(db_err)?;
            return Err(AuthorityError::new(ErrorCode::Conflict, "request passed its deadline and was rejected", Some(rid)));
        }

        match body.decision {
            Decision::Approve => {
                let ttl = stored.token_ttl_secs.min(tokens::MAX_TTL_SECS);
                let claims = TokenClaims {
                    token_id: format!("tok_{}", uuid::Uuid::new_v4().simple()),
                    request_id: body.request_id.clone(),
                    action_class: req.action_class,
                    action_hash: req.action_hash.clone().unwrap_or_default(),
                    principal: req.principal.clone(),
                    approved_by: caller.principal.clone(),
                    issued_at: now,
                    expires_at: now + Duration::seconds(ttl as i64),
                };
                let token = tokens::mint(&self.signer, &claims)?;
                payload["event"] = json!("approve");
                payload["token_id"] = json!(claims.token_id);
                payload["action_hash"] = json!(claims.action_hash);
                payload["expires_at"] = json!(token.expires_at);
                append_in_tx(&tx, &self.signer, self.input(&chain, &caller.principal, "approve", payload), now)?;
                if open_guard(
                    &tx,
                    "UPDATE approval_requests SET status = 'approved', resolved_at = ?2, resolved_by = ?3, resolve_note = ?4, token_id = ?5
                     WHERE request_id = ?1 AND status = 'open'",
                    &[&body.request_id, &resolved_at, &caller.principal, &body.note, &claims.token_id],
                )
                .is_err()
                {
                    drop(tx);
                    return Err(self.deny(&caller.principal, &chain, ErrorCode::Conflict, "request changed while resolving", ctx));
                }
                tx.commit().map_err(db_err)?;
                Ok(Resolution::Approved(token))
            }
            Decision::Deny => {
                payload["event"] = json!("deny");
                append_in_tx(&tx, &self.signer, self.input(&chain, &caller.principal, "deny", payload), now)?;
                if open_guard(
                    &tx,
                    "UPDATE approval_requests SET status = 'denied', resolved_at = ?2, resolved_by = ?3, resolve_note = ?4
                     WHERE request_id = ?1 AND status = 'open'",
                    &[&body.request_id, &resolved_at, &caller.principal, &body.note],
                )
                .is_err()
                {
                    drop(tx);
                    return Err(self.deny(&caller.principal, &chain, ErrorCode::Conflict, "request changed while resolving", ctx));
                }
                tx.commit().map_err(db_err)?;
                let mut out = stored.request;
                out.status = Some(ApprovalStatus::Denied);
                out.resolved_at = Some(resolved_at);
                out.resolved_by = Some(caller.principal.clone());
                Ok(Resolution::Denied(out))
            }
        }
    }

    /// `approval.redeem`: verify, then spend the token and write the redemption receipt in one
    /// transaction, before the action runs. The `approval_token_spends` primary key makes a
    /// second spend impossible even if two redeems race.
    pub fn redeem(&self, caller: &Caller, body: ApprovalRedeemBody) -> Result<ApprovalRedeemResult, AuthorityError> {
        let now = self.now();
        if !tokens::is_content_hash(&body.action_hash) {
            return Err(bad("action_hash must be sha256:<64 lowercase hex>"));
        }
        let fallback_chain = format!("kernel:{}", caller.principal);
        let mut ctx = json!({
            "op": "redeem", "action_hash": body.action_hash, "caller_kind": enum_str(caller.kind), "caller": caller.principal,
        });

        let claims = match tokens::verify(&self.signer, &body.token) {
            Ok(c) => c,
            Err(reason) => return Err(self.deny(&caller.principal, &fallback_chain, ErrorCode::TokenInvalid, reason, ctx)),
        };
        ctx["token_id"] = json!(claims.token_id);
        ctx["request_id"] = json!(claims.request_id);

        let conn = self.connect()?;
        let stored = self.load(&conn, &claims.request_id)?;
        let spent: bool = conn
            .query_row("SELECT 1 FROM approval_token_spends WHERE token_id = ?1", params![claims.token_id], |_| Ok(()))
            .optional()
            .map_err(db_err)?
            .is_some();
        drop(conn);

        let Some(stored) = stored else {
            return Err(self.deny(&caller.principal, &fallback_chain, ErrorCode::TokenInvalid, "token names no known request", ctx));
        };
        let chain = stored.request.run_id.clone();
        if stored.owner_user_id != caller.principal {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::ForbiddenCaller, "token belongs to another account", ctx));
        }
        if stored.request.status != Some(ApprovalStatus::Approved) || stored.token_id.as_deref() != Some(claims.token_id.as_str()) {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::TokenInvalid, "token was not issued for this request", ctx));
        }
        if spent {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::TokenReplayed, "token already spent", ctx));
        }
        if now > claims.expires_at {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::TokenExpired, "token expired", ctx));
        }
        if claims.action_hash != body.action_hash || stored.request.action_hash.as_deref() != Some(body.action_hash.as_str()) {
            return Err(self.deny(&caller.principal, &chain, ErrorCode::HashMismatch, "token is bound to a different payload", ctx));
        }

        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        let mut payload = ctx.clone();
        payload["event"] = json!("redeem");
        let receipt_id =
            append_in_tx(&tx, &self.signer, self.input(&chain, &caller.principal, "redeem", payload), now)?;
        let inserted = tx.execute(
            "INSERT INTO approval_token_spends (token_id, request_id, action_hash, redeemed_by, spend_receipt_id, spent_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![claims.token_id, claims.request_id, body.action_hash, caller.principal, receipt_id, tokens::ts(now)],
        );
        match inserted {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
                drop(tx);
                return Err(self.deny(&caller.principal, &chain, ErrorCode::TokenReplayed, "token already spent", ctx));
            }
            Err(e) => return Err(db_err(e)),
        }
        tx.commit().map_err(db_err)?;
        Ok(ApprovalRedeemResult { redeemed: true, request_id: claims.request_id, spend_receipt_id: receipt_id })
    }
}

/// Run a guarded `UPDATE ... WHERE status = 'open'`; anything but one row means the request moved.
fn open_guard(tx: &Connection, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<(), AuthorityError> {
    match tx.execute(sql, p).map_err(db_err)? {
        1 => Ok(()),
        _ => Err(AuthorityError::without_receipt(ErrorCode::Conflict, "request changed while resolving")),
    }
}

fn not_yet(call: &str, step: u8) -> AuthorityError {
    AuthorityError::kernel_unavailable(format!("{call} is not wired in allternit-api yet (plan step {step}); fail closed"))
}

/// The crate's trait. The approval calls are real; the rest fail closed (`kernel_unavailable`,
/// which every caller treats as DENY) until their plan steps land.
impl Authority for SqlAuthority {
    fn check(&mut self, _: &Caller, _: PolicyCheck) -> Result<PolicyDecision, AuthorityError> {
        Err(not_yet("check", 9))
    }
    fn lease(&mut self, _: &Caller, _: LeaseRequest) -> Result<LeaseResponse, AuthorityError> {
        Err(not_yet("lease", 12))
    }
    fn secret_use(&mut self, _: &Caller, _: SecretUseV1) -> Result<SecretUseResult, AuthorityError> {
        Err(not_yet("secret.use", 10))
    }
    fn approval_request(&mut self, caller: &Caller, body: ApprovalRequestV1) -> Result<ApprovalRequestV1, AuthorityError> {
        self.request(caller, body)
    }
    fn approval_redeem(&mut self, caller: &Caller, body: ApprovalRedeemBody) -> Result<ApprovalRedeemResult, AuthorityError> {
        self.redeem(caller, body)
    }
    fn audit_append(&mut self, _: &Caller, _: AuditAppendBody) -> Result<ReceiptChain, AuthorityError> {
        Err(not_yet("audit.append", 8))
    }
    fn budget_reserve(&mut self, _: &Caller, _: BudgetReservationV1) -> Result<BudgetReservationV1, AuthorityError> {
        Err(not_yet("budget.reserve", 12))
    }
    fn budget_commit(&mut self, _: &Caller, _: BudgetCommitBody) -> Result<BudgetReservationV1, AuthorityError> {
        Err(not_yet("budget.commit", 12))
    }
    fn budget_release(&mut self, _: &Caller, _: BudgetReleaseBody) -> Result<BudgetReservationV1, AuthorityError> {
        Err(not_yet("budget.release", 12))
    }
    fn deploy_admit(&mut self, _: &Caller, _: DeployAdmissionV1) -> Result<DeployAdmissionV1, AuthorityError> {
        Err(not_yet("deploy.admit", 12))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "user_owner";
    const AGENT: &str = "agent_1";

    fn t0() -> DateTime<Utc> {
        tokens::parse_ts("2026-11-28T12:00:00Z").unwrap()
    }

    fn kernel() -> SqlAuthority {
        let db = DbHandle::new_memory().expect("db");
        SqlAuthority::new(db, Arc::new(ReceiptSigner::from_seed([7u8; 32]))).at(t0())
    }

    fn human() -> Caller {
        Caller { kind: CallerKind::Human, principal: OWNER.into() }
    }

    fn run() -> Caller {
        Caller { kind: CallerKind::Run, principal: OWNER.into() }
    }

    fn payload(amount: u64) -> Map<String, Value> {
        json!({ "tool": "stripe_send_invoice", "invoice": "in_1", "amount_cents": amount }).as_object().unwrap().clone()
    }

    fn body(ttl: Option<u64>) -> ApprovalRequestV1 {
        ApprovalRequestV1 {
            schema_id: REQUEST_SCHEMA_ID.into(),
            schema_version: "1.0.0".into(),
            request_id: None,
            source_store: SourceStore::Kernel,
            source_ref: None,
            run_id: "run_1".into(),
            principal: AGENT.into(),
            assignee: "owner".into(),
            action_class: ActionClass::Money,
            action_summary: "Send invoice in_1 for $10".into(),
            action_payload: payload(1000),
            action_hash: None,
            deadline: "2026-11-28T13:00:00Z".into(),
            on_expiry: "reject".into(),
            attention_id: None,
            check_id: "chk_1".into(),
            status: None,
            created_at: None,
            resolved_at: None,
            resolved_by: None,
            evidence_refs: None,
            extensions: ttl.map(|t| json!({ OS_EXTENSION_KEY: { "token_ttl_secs": t } }).as_object().unwrap().clone()),
        }
    }

    fn approved(k: &SqlAuthority, ttl: Option<u64>) -> (ApprovalRequestV1, ApprovalTokenV1) {
        let req = k.request(&run(), body(ttl)).expect("request");
        let resolved = k
            .resolve(&human(), ResolveBody { request_id: req.request_id.clone().unwrap(), decision: Decision::Approve, note: None })
            .expect("resolve");
        let Resolution::Approved(token) = resolved else { panic!("expected a token") };
        (req, token)
    }

    fn count(k: &SqlAuthority, sql: &str) -> i64 {
        k.db.connect().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn approve_without_presence_is_refused_with_a_receipt_and_the_request_stays_open() {
        let k = kernel();
        let req = k.request(&run(), body(None)).expect("request");
        let id = req.request_id.clone().unwrap();
        let resolve = ResolveBody { request_id: id.clone(), decision: Decision::Approve, note: None };
        let err = k.refuse_without_presence(&human(), &resolve);
        assert_eq!(err.code, ErrorCode::ForbiddenCaller);
        assert!(err.receipt_id.is_some(), "the refusal writes a receipt");
        assert_eq!(count(&k, "SELECT COUNT(*) FROM approval_requests WHERE status = 'open'"), 1);
        // The person can still approve once presence passes.
        assert!(matches!(k.resolve(&human(), resolve).unwrap(), Resolution::Approved(_)));
    }

    #[test]
    fn redeem_with_a_different_payload_hash_is_denied() {
        let k = kernel();
        let (req, token) = approved(&k, None);
        let other = tokens::action_hash(&payload(999_999)).unwrap();
        assert_ne!(Some(other.clone()), req.action_hash);
        let err = k.redeem(&run(), ApprovalRedeemBody { token: token.token.clone(), action_hash: other }).unwrap_err();
        assert_eq!(err.code, ErrorCode::HashMismatch);
        assert!(err.receipt_id.is_some(), "a denial writes a receipt");
        assert_eq!(count(&k, "SELECT COUNT(*) FROM approval_token_spends"), 0, "nothing spent");
        // The right payload still redeems afterwards.
        let ok = k.redeem(&run(), ApprovalRedeemBody { token: token.token, action_hash: req.action_hash.unwrap() }).unwrap();
        assert!(ok.redeemed);
    }

    #[test]
    fn second_redeem_of_one_token_is_denied() {
        let k = kernel();
        let (req, token) = approved(&k, None);
        let redeem = ApprovalRedeemBody { token: token.token, action_hash: req.action_hash.unwrap() };
        let first = k.redeem(&run(), redeem.clone()).expect("first redeem");
        assert_eq!(first.request_id, req.request_id.unwrap());
        let err = k.redeem(&run(), redeem).unwrap_err();
        assert_eq!(err.code, ErrorCode::TokenReplayed);
        assert!(err.receipt_id.is_some());
        assert_eq!(count(&k, "SELECT COUNT(*) FROM approval_token_spends"), 1);
    }

    #[test]
    fn redeem_after_expiry_is_denied_and_ttl_is_capped_at_900s() {
        let k = kernel();
        let (req, token) = approved(&k, Some(60));
        let late = SqlAuthority { db: k.db.clone(), signer: k.signer.clone(), now: Some(t0() + Duration::seconds(61)) };
        let err = late.redeem(&run(), ApprovalRedeemBody { token: token.token, action_hash: req.action_hash.unwrap() }).unwrap_err();
        assert_eq!(err.code, ErrorCode::TokenExpired);
        assert!(err.receipt_id.is_some());

        let (_, long) = approved(&k, Some(5_000));
        let life = tokens::parse_ts(&long.expires_at).unwrap() - tokens::parse_ts(&long.issued_at).unwrap();
        assert_eq!(life.num_seconds(), 900, "TTL is capped at 900 s whatever the request asks");
        let (_, default) = approved(&k, None);
        let life = tokens::parse_ts(&default.expires_at).unwrap() - tokens::parse_ts(&default.issued_at).unwrap();
        assert_eq!(life.num_seconds(), 300);
    }

    #[test]
    fn agent_credential_cannot_resolve() {
        let k = kernel();
        let req = k.request(&run(), body(None)).unwrap();
        let id = req.request_id.unwrap();
        for caller in [run(), Caller { kind: CallerKind::Service, principal: OWNER.into() }] {
            let err = k.resolve(&caller, ResolveBody { request_id: id.clone(), decision: Decision::Approve, note: None }).unwrap_err();
            assert_eq!(err.code, ErrorCode::ForbiddenCaller);
            assert!(err.receipt_id.is_some(), "a refused resolve still writes a receipt");
        }
        let status: String = k
            .db
            .connect()
            .unwrap()
            .query_row("SELECT status FROM approval_requests WHERE request_id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "open", "a refused resolve changes nothing");
        // The acting principal cannot approve its own request even from a human session.
        let selfish = Caller { kind: CallerKind::Human, principal: AGENT.into() };
        let mut own = body(None);
        own.principal = AGENT.into();
        let own = k.request(&selfish, own).unwrap();
        let err = k.resolve(&selfish, ResolveBody { request_id: own.request_id.unwrap(), decision: Decision::Approve, note: None }).unwrap_err();
        assert_eq!(err.code, ErrorCode::ForbiddenCaller);
    }

    #[test]
    fn tampered_token_is_invalid_and_receipts_are_append_only() {
        let k = kernel();
        let (req, token) = approved(&k, None);
        let forged_claims = TokenClaims {
            token_id: token.token_id.clone(),
            request_id: token.request_id.clone(),
            action_class: ActionClass::Money,
            action_hash: token.action_hash.clone(),
            principal: AGENT.into(),
            approved_by: OWNER.into(),
            issued_at: t0(),
            expires_at: t0() + Duration::seconds(300),
        };
        let forged = tokens::mint(&ReceiptSigner::from_seed([8u8; 32]), &forged_claims).unwrap();
        let err = k.redeem(&run(), ApprovalRedeemBody { token: forged.token, action_hash: req.action_hash.unwrap() }).unwrap_err();
        assert_eq!(err.code, ErrorCode::TokenInvalid);

        let conn = k.db.connect().unwrap();
        assert!(conn.execute("UPDATE kernel_receipts SET payload = '{}'", []).is_err());
        assert!(conn.execute("DELETE FROM kernel_receipts", []).is_err());
        let first_prev: String = conn
            .query_row("SELECT prev_hash FROM kernel_receipts WHERE run_id = 'run_1' AND idx = 0", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first_prev, super::super::GENESIS_HASH);
    }
}
