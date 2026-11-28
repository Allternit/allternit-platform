//! Kernel adapter over cloud-api's approval store, `platform_approvals` (AllternitOS Stage 1,
//! step 11b; CONTRACT.md 3.1, 3.2, 4.3 and section 6: every old store gets an adapter, no tenth
//! store).
//!
//! A tool call cloud-api runs itself (a chat with no computer: `mcp_edge`, `agent_tools`) that
//! [`crate::kernel::check`] does not allow opens an approval request here:
//!
//! * **Request.** A `platform_approvals` row with `source = 'kernel'`, `source_ref` = its own id,
//!   `approver_user_id` = the owner (assignee), the action hash and class, the principal and run,
//!   the check receipt, a deadline of `ALLTERNIT_KERNEL_APPROVAL_TTL_SECS` (default 300 s, cap
//!   900 s) and `proposed_action` = `{tool, surface, args, summary, human_only, run_id}` (the
//!   exact action, shown to the person). In enforce mode it is `pending` and pushed to the
//!   owner's phones once; repeating the same call while it is open reuses it. In shadow mode it
//!   is a dry record (`status = 'shadow'`): never pending, never listed, never pushed.
//! * **Phone list.** [`pending_items`] adds the owner's open kernel rows to
//!   `GET /api/v1/kernel/relay/approvals` with `runtimeId = "cloud"` and
//!   `resolveRoute = /api/v1/kernel/relay/approvals/cloud/<id>/resolve`, the route the phones
//!   already call for device requests (iOS #3309, Android #3310), so they approve these with no
//!   change.
//! * **Resolve.** [`resolve_cloud`], reached from that route only after the relay's own checks
//!   (no machine credential, a person's Clerk session; API keys are refused). Approve mints a
//!   single-use `ApprovalTokenV1` with cloud-api's kernel key (`ALLTERNIT_KERNEL_SEED`), bound to
//!   the action hash and the requesting principal, TTL as above, and stores it on the row. The
//!   Platform API's `POST /v1/approvals/{id}` refuses kernel rows with a receipt: a developer's
//!   API key never approves a kernel request.
//! * **Redeem.** The retried call redeems the token ([`redeem`]): the one sent as
//!   `kernel_approval_token` (taken out of the arguments before hashing, [`split_token`]) or, when
//!   none is sent, the token stored on this principal's approved row for the same action hash.
//!   Spending is `approved -> consumed`, once; a second use is refused (`approval_token_spent`).
//! * **Existing holds.** A tool with its own human hold does not ask twice: the call path sets
//!   `Tool::approved_by` (phone SMS/dial wait on the owner's Desktop phone guard; a retry with an
//!   approved `hold_id`; a platform-agent text with an approved twin `approval_id`), and
//!   [`record_hold`] writes the hold's resolution onto the owner's cloud receipt chain.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::kernel::{self, canonical, KernelKey, Mode, ReceiptIn, Tool};

pub const SOURCE: &str = "kernel";
/// The `runtimeId` kernel rows carry in the phone list (no device holds them).
pub const CLOUD_RUNTIME: &str = "cloud";
pub const TOKEN_FIELD: &str = "kernel_approval_token";
pub const TTL_ENV: &str = "ALLTERNIT_KERNEL_APPROVAL_TTL_SECS";
/// CONTRACT.md 3.2 / 12.1.
pub const DEFAULT_TTL_SECS: i64 = 300;
pub const MAX_TTL_SECS: i64 = 900;
const TOKEN_SCHEMA_ID: &str = "allternit.os.ApprovalTokenV1";
const MAX_SUMMARY: usize = 280;
const MAX_TOKEN_CHARS: usize = 4096;

/// Where a call's approval request goes.
#[derive(Clone, Copy, Debug)]
pub struct AskTo<'a> {
    /// `platform_approvals.project_id` (a real project: the key's or the agent's).
    pub project_id: &'a str,
    /// Who must approve. `None`: the project's owner.
    pub assignee: Option<&'a str>,
}

/// Request and token TTL: the env value, default 300 s, clamped to 1..=900 s.
pub fn ttl_secs() -> i64 {
    std::env::var(TTL_ENV).ok().and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(DEFAULT_TTL_SECS).clamp(1, MAX_TTL_SECS)
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Take `kernel_approval_token` out of a tool's arguments: it is never hashed and the tool never
/// sees it.
pub fn split_token(args: &Value) -> (Value, Option<String>) {
    match args {
        Value::Object(m) if m.contains_key(TOKEN_FIELD) => {
            let mut m = m.clone();
            let token = m
                .remove(TOKEN_FIELD)
                .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty() && s.len() <= MAX_TOKEN_CHARS);
            (Value::Object(m), token)
        }
        other => (other.clone(), None),
    }
}

// ---------------------------------------------------------------- tokens

/// The verified claims of a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    pub token_id: String,
    pub request_id: String,
    pub action_hash: String,
    pub principal: String,
    pub approved_by: String,
    pub expires_at: DateTime<Utc>,
}

/// Mint an `ApprovalTokenV1` (CONTRACT.md 3.2): `signature` over the canonical JSON of every
/// other field except `token`; `token` = base64url(canonical JSON of those fields plus
/// `signature`). Returns the full token object.
pub fn mint(key: &KernelKey, request_id: &str, action_class: &str, action_hash: &str, principal: &str, approved_by: &str, now: DateTime<Utc>) -> Value {
    let fields = json!({
        "schema_id": TOKEN_SCHEMA_ID, "schema_version": kernel::SCHEMA_VERSION,
        "token_id": format!("tok_{}", uuid::Uuid::new_v4().simple()), "request_id": request_id,
        "action_class": action_class, "action_hash": action_hash, "principal": principal, "approved_by": approved_by,
        "issued_at": ts(now), "expires_at": ts(now + Duration::seconds(ttl_secs())), "single_use": true, "key_id": key.kid(),
    });
    let signature = key.sign(canonical(&fields).as_bytes());
    let mut signed = fields;
    signed["signature"] = json!(signature);
    let token = URL_SAFE_NO_PAD.encode(canonical(&signed));
    signed["token"] = json!(token);
    signed
}

/// Check a token's signature (this kernel's key), schema, `single_use` and expiry. The caller
/// still compares the action hash and principal and spends it.
pub fn verify(key: &KernelKey, token: &str, now: DateTime<Utc>) -> Result<Claims, &'static str> {
    let bytes = URL_SAFE_NO_PAD.decode(token.trim()).map_err(|_| "approval_token_invalid")?;
    let Ok(Value::Object(mut m)) = serde_json::from_slice::<Value>(&bytes) else { return Err("approval_token_invalid") };
    let sig = m.remove("signature").and_then(|v| v.as_str().map(str::to_string)).ok_or("approval_token_invalid")?;
    let fields = Value::Object(m.clone());
    if m.get("schema_id").and_then(Value::as_str) != Some(TOKEN_SCHEMA_ID)
        || m.get("key_id").and_then(Value::as_str) != Some(key.kid())
        || m.get("single_use") != Some(&Value::Bool(true))
        || !key.verify(canonical(&fields).as_bytes(), &sig)
    {
        return Err("approval_token_invalid");
    }
    let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let expires_at = DateTime::parse_from_rfc3339(&s("expires_at")).map_err(|_| "approval_token_invalid")?.with_timezone(&Utc);
    if expires_at <= now {
        return Err("approval_token_expired");
    }
    Ok(Claims { token_id: s("token_id"), request_id: s("request_id"), action_hash: s("action_hash"), principal: s("principal"), approved_by: s("approved_by"), expires_at })
}

/// A token spent by this call.
#[derive(Clone, Debug)]
pub struct Redeemed {
    pub token_id: String,
    pub request_id: String,
    pub approved_by: String,
}

pub enum Redeem {
    /// No token was sent and none is waiting for this action.
    None,
    Ok(Redeemed),
    /// A token was sent (or found) and refused: the code for the refusal.
    Refused(&'static str),
}

/// Redeem a token for `(principal, action_hash)`: the one sent, else the one minted on this
/// principal's approved row for the same action. Spends it (`approved -> consumed`) at most once.
pub async fn redeem(db: &PgPool, key: &KernelKey, principal: &str, action_hash: &str, presented: Option<&str>) -> Redeem {
    let token = match presented {
        Some(t) => t.to_string(),
        None => {
            let found: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
                "SELECT kernel_token FROM platform_approvals WHERE source = 'kernel' AND kernel_principal = $1 AND action_hash = $2 \
                   AND status = 'approved' AND kernel_token IS NOT NULL AND kernel_token_expires_at > NOW() \
                 ORDER BY resolved_at DESC LIMIT 1",
            )
            .bind(principal)
            .bind(action_hash)
            .fetch_optional(db)
            .await;
            match found {
                Ok(Some(t)) => t,
                Ok(None) => return Redeem::None,
                Err(e) => {
                    tracing::warn!("kernel approvals: token lookup failed ({e})");
                    return Redeem::None;
                }
            }
        }
    };
    let claims = match verify(key, &token, Utc::now()) {
        Ok(c) => c,
        Err(code) => return Redeem::Refused(code),
    };
    if claims.action_hash != action_hash || claims.principal != principal {
        return Redeem::Refused("approval_hash_mismatch");
    }
    let spent: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "UPDATE platform_approvals SET status = 'consumed', kernel_redeemed_at = NOW() \
          WHERE source = 'kernel' AND kernel_token_id = $1 AND status = 'approved' RETURNING id",
    )
    .bind(&claims.token_id)
    .fetch_optional(db)
    .await;
    match spent {
        Ok(Some(id)) => Redeem::Ok(Redeemed { token_id: claims.token_id, request_id: id, approved_by: claims.approved_by }),
        Ok(None) => Redeem::Refused("approval_token_spent"),
        Err(e) => {
            tracing::warn!("kernel approvals: token not spent ({e})");
            Redeem::Refused("approval_token_spent")
        }
    }
}

/// Link the check receipt that redeemed a token to its row (best effort).
pub async fn note_redemption(db: &PgPool, token_id: &str, receipt_id: &str) {
    let _ = sqlx::query("UPDATE platform_approvals SET kernel_redemption_receipt_id = $2 WHERE kernel_token_id = $1")
        .bind(token_id)
        .bind(receipt_id)
        .execute(db)
        .await;
}

// ---------------------------------------------------------------- requests

/// The id the call's request will have: an open request for the same action (enforce), or a new one.
pub struct RequestId {
    pub id: String,
    pub existing: bool,
}

pub async fn request_id_for(db: &PgPool, mode: Mode, to: &AskTo<'_>, principal: &str, action_hash: &str) -> Option<RequestId> {
    if mode == Mode::Enforce {
        let open: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
            "SELECT id FROM platform_approvals WHERE source = 'kernel' AND project_id = $1 AND kernel_principal = $2 AND action_hash = $3 \
               AND status = 'pending' AND expires_at > NOW() ORDER BY created_at DESC LIMIT 1",
        )
        .bind(to.project_id)
        .bind(principal)
        .bind(action_hash)
        .fetch_optional(db)
        .await;
        match open {
            Ok(Some(id)) => return Some(RequestId { id, existing: true }),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("kernel approvals: store unavailable ({e})");
                return None;
            }
        }
    }
    Some(RequestId { id: crate::routes::platform_v1::new_id("apr_"), existing: false })
}

/// Plain words for the person: the tool and its exact arguments, clipped.
pub fn summary(tool: &Tool<'_>, class: &str) -> String {
    let args = canonical(tool.args);
    let mut s = format!("{} ({class}): {args}", tool.name);
    if s.chars().count() > MAX_SUMMARY {
        s = s.chars().take(MAX_SUMMARY - 1).collect::<String>() + "\u{2026}";
    }
    s
}

/// Everything [`open_request`] writes.
pub struct NewRequest<'a> {
    pub mode: Mode,
    pub to: AskTo<'a>,
    pub principal: &'a str,
    pub run_id: &'a str,
    pub tool: &'a Tool<'a>,
    pub action_hash: &'a str,
    pub action_class: &'a str,
    pub decision: &'a str,
    pub receipt_id: &'a str,
}

/// The CONTRACT 6.4 `kernel` object for a row (what the phone shows).
pub fn kernel_object(id: &str, run_id: &str, class: &str, summary: &str, hash: &str, deadline: DateTime<Utc>, human_only: bool) -> Value {
    json!({
        "requestId": id, "runId": run_id, "assignee": "owner", "actionClass": class, "actionSummary": summary,
        "actionHash": hash, "deadline": ts(deadline), "humanOnly": human_only,
    })
}

/// Write the request row (enforce: pending, pushed once; shadow: a dry record). Returns its id,
/// or `None` when it could not be written (the refusal then has no id; enforce still refuses).
pub async fn open_request(db: &PgPool, n: NewRequest<'_>, req: RequestId) -> Option<String> {
    if req.existing {
        return Some(req.id);
    }
    let assignee = match n.to.assignee {
        Some(a) => a.to_string(),
        None => match sqlx::query_scalar::<_, String>("SELECT owner_user_id FROM platform_projects WHERE id = $1")
            .bind(n.to.project_id)
            .fetch_optional(db)
            .await
        {
            Ok(Some(o)) => o,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("kernel approvals: project owner lookup failed ({e})");
                return None;
            }
        },
    };
    let human_only = n.decision == "human_only";
    let summary = summary(n.tool, n.action_class);
    let deadline = Utc::now() + Duration::seconds(ttl_secs());
    let shadow = n.mode == Mode::Shadow;
    let proposed = json!({
        "tool": n.tool.name, "surface": n.tool.surface, "args": n.tool.args, "summary": summary,
        "human_only": human_only, "run_id": n.run_id, "approver_user_id": assignee,
    });
    let inserted = sqlx::query(
        "INSERT INTO platform_approvals (id, project_id, source, source_ref, kind, proposed_action, status, expires_at, resolved_at, \
             approver_user_id, actor_ref, action_hash, action_class, kernel_principal, kernel_run_id, kernel_mode, kernel_check_receipt_id) \
         VALUES ($1, $2, 'kernel', $1, $3, $4, $5, $6, CASE WHEN $7 THEN NOW() END, $8, $9, $10, $11, $9, $12, $13, $14)",
    )
    .bind(&req.id)
    .bind(n.to.project_id)
    .bind(&summary)
    .bind(&proposed)
    .bind(if shadow { "shadow" } else { "pending" })
    .bind(deadline)
    .bind(shadow)
    .bind(&assignee)
    .bind(n.principal)
    .bind(n.action_hash)
    .bind(n.action_class)
    .bind(n.run_id)
    .bind(if shadow { "shadow" } else { "enforce" })
    .bind(n.receipt_id)
    .execute(db)
    .await;
    if let Err(e) = inserted {
        tracing::warn!("kernel approvals: request not written ({e})");
        return None;
    }
    if !shadow {
        // First sight: the same push as a device request (category KERNEL_APPROVAL), off the
        // tool path so a slow push never holds the call.
        let k = kernel_object(&req.id, n.run_id, n.action_class, &summary, n.action_hash, deadline, human_only);
        let msg = crate::routes::kernel_relay::push_message(CLOUD_RUNTIME, &k);
        let db = db.clone();
        tokio::spawn(async move { crate::routes::aci_push::deliver(&db, &assignee, &msg).await });
    }
    Some(req.id)
}

/// The owner's open kernel requests as phone inbox rows (the relay's item shape, `runtimeId`
/// `"cloud"`).
pub async fn pending_items(db: &PgPool, owner: &str) -> Result<Vec<Value>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, Option<String>, Option<String>, String, Option<String>, DateTime<Utc>, DateTime<Utc>, Value)> = sqlx::query_as(
        "SELECT id, kernel_run_id, action_class, kind, action_hash, expires_at, created_at, proposed_action FROM platform_approvals \
          WHERE source = 'kernel' AND kernel_mode = 'enforce' AND approver_user_id = $1 AND status = 'pending' AND expires_at > NOW() \
          ORDER BY created_at DESC LIMIT 100",
    )
    .bind(owner)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, run_id, class, summary, hash, deadline, created_at, proposed)| {
            let human_only = proposed["human_only"].as_bool().unwrap_or(true);
            let k = kernel_object(&id, run_id.as_deref().unwrap_or(""), class.as_deref().unwrap_or(""), &summary, hash.as_deref().unwrap_or(""), deadline, human_only);
            json!({
                "id": format!("kernel:{id}"), "kind": "kernel_approval", "source": "kernel",
                "title": summary, "createdAt": created_at, "attentionId": Value::Null,
                "runtimeId": CLOUD_RUNTIME, "actions": ["approve", "deny"],
                "resolveRoute": resolve_route(&id),
                "kernel": k,
            })
        })
        .collect())
}

pub fn resolve_route(id: &str) -> String {
    format!("{}/{CLOUD_RUNTIME}/{id}/resolve", crate::routes::kernel_relay::APPROVALS_PATH)
}

/// The approver of a kernel row (for a refusal receipt on their chain).
pub async fn approver_of(db: &PgPool, id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT approver_user_id FROM platform_approvals WHERE id = $1 AND source = 'kernel'")
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// A refused resolve attempt (machine credential, API key) on the approver's chain.
pub async fn refuse_receipt(db: &PgPool, id: &str, reason: &str) {
    let Some(owner) = approver_of(db, id).await else { return };
    let Ok(key) = kernel::key_from_env() else { return };
    let _ = kernel::append(
        db,
        &key,
        ReceiptIn { run_id: id, principal: &owner, kind: "resolve_refused", schema_id: "allternit.os.ApprovalResolve", decision: "forbidden_caller", payload: json!({ "request_id": id, "reason": reason }), idempotency_key: None },
    )
    .await;
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": { "code": code, "message": message } }))).into_response()
}

/// Resolve a kernel row for `user` (a person's Clerk session, already checked by the caller;
/// see `routes::kernel_relay::resolve_for`). Approve returns the minted token, deny the request.
pub async fn resolve_cloud(db: &PgPool, user: &str, request_id: &str, decision: &str, note: Option<&str>) -> Response {
    #[allow(clippy::type_complexity)]
    let row: Result<Option<(String, Option<DateTime<Utc>>, Option<String>, Option<String>, Option<String>, Option<String>)>, sqlx::Error> = sqlx::query_as(
        "SELECT status, expires_at, action_hash, action_class, kernel_principal, kernel_run_id FROM platform_approvals \
          WHERE id = $1 AND source = 'kernel' AND kernel_mode = 'enforce' AND approver_user_id = $2",
    )
    .bind(request_id)
    .bind(user)
    .fetch_optional(db)
    .await;
    let (status, expires_at, hash, class, principal, run_id) = match row {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "not_found", "request not found"),
        Err(e) => {
            tracing::error!("kernel approvals: read failed: {e}");
            return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "approval store unavailable");
        }
    };
    let run_id = run_id.unwrap_or_default();
    if status == "pending" && expires_at.is_some_and(|d| d <= Utc::now()) {
        let _ = sqlx::query("UPDATE platform_approvals SET status = 'expired', resolved_at = NOW() WHERE id = $1 AND status = 'pending'")
            .bind(request_id)
            .execute(db)
            .await;
        return err(StatusCode::CONFLICT, "token_expired", "this request expired");
    }
    if status != "pending" {
        return err(StatusCode::CONFLICT, "conflict", "this request was already decided");
    }
    let key = match kernel::key_from_env() {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!("kernel approvals: resolve refused, no key ({e})");
            return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "kernel key unavailable");
        }
    };
    // Write before act.
    let note: Option<String> = note.map(|n| n.chars().take(500).collect());
    let receipt = kernel::append(
        db,
        &key,
        ReceiptIn { run_id: &run_id, principal: user, kind: "resolve", schema_id: "allternit.os.ApprovalResolve", decision, payload: json!({ "request_id": request_id, "store": "platform_approvals", "note": note }), idempotency_key: None },
    )
    .await;
    if let Err(e) = receipt {
        tracing::warn!("kernel approvals: resolve refused, receipt not written ({e})");
        return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "kernel receipt log unavailable");
    }
    if decision == "deny" {
        let moved = sqlx::query("UPDATE platform_approvals SET status = 'denied', resolved_at = NOW(), resolved_by = $2 WHERE id = $1 AND status = 'pending'")
            .bind(request_id)
            .bind(user)
            .execute(db)
            .await;
        return match moved {
            Ok(r) if r.rows_affected() == 1 => (StatusCode::OK, Json(json!({ "ok": true, "request": { "requestId": request_id, "status": "denied" } }))).into_response(),
            Ok(_) => err(StatusCode::CONFLICT, "conflict", "this request was already decided"),
            Err(_) => err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "approval store unavailable"),
        };
    }
    let token = mint(&key, request_id, class.as_deref().unwrap_or(""), hash.as_deref().unwrap_or(""), principal.as_deref().unwrap_or(""), user, Utc::now());
    let expires = token["expires_at"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc));
    let moved = sqlx::query(
        "UPDATE platform_approvals SET status = 'approved', resolved_at = NOW(), resolved_by = $2, kernel_token_id = $3, kernel_token = $4, \
             kernel_token_expires_at = $5 WHERE id = $1 AND status = 'pending'",
    )
    .bind(request_id)
    .bind(user)
    .bind(token["token_id"].as_str())
    .bind(token["token"].as_str())
    .bind(expires)
    .execute(db)
    .await;
    match moved {
        Ok(r) if r.rows_affected() == 1 => (StatusCode::OK, Json(json!({ "ok": true, "token": token }))).into_response(),
        Ok(_) => err(StatusCode::CONFLICT, "conflict", "this request was already decided"),
        Err(_) => err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "approval store unavailable"),
    }
}

/// Write-through for a tool's own human hold (phone guard on the owner's Desktop): its
/// resolution goes on the owner's cloud chain, once per (hold, state).
pub async fn record_hold(db: &PgPool, owner: &str, hold: &str, data: &Value) {
    let Some(id) = data.get("approval_id").or_else(|| data.get("approvalId")).and_then(Value::as_str).filter(|s| !s.is_empty()) else { return };
    let state = data.get("state").and_then(Value::as_str).unwrap_or("resolved");
    let Ok(key) = kernel::key_from_env() else { return };
    let mut payload = Map::new();
    payload.insert("hold".into(), json!(hold));
    payload.insert("approval_id".into(), json!(id));
    payload.insert("effect".into(), data.get("effect").cloned().unwrap_or(Value::Null));
    let run_id = data.get("run_id").and_then(Value::as_str).unwrap_or(id).to_string();
    let idem = format!("{hold}:{id}:{state}");
    if let Err(e) = kernel::append(
        db,
        &key,
        ReceiptIn { run_id: &run_id, principal: owner, kind: "hold_resolved", schema_id: "allternit.os.ApprovalHold", decision: state, payload: Value::Object(payload), idempotency_key: Some(&idem) },
    )
    .await
    {
        tracing::warn!("kernel approvals: hold receipt not written ({e})");
    }
}

#[cfg(test)]
#[path = "kernel_approvals_tests.rs"]
mod tests;
