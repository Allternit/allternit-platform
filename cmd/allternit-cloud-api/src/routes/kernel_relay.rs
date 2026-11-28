//! Kernel approval relay to phones (AllternitOS Stage 1, plan step 18; CONTRACT.md 4.3, 6.4).
//!
//! A person's Desktop or cloud computer (allternit-api) raises a kernel approval request for its
//! owner (`POST /api/v1/kernel/approvals/request` there). It sends the request up here; cloud-api
//! pushes it to the owner's phones, lists it in the phone's feed, and forwards the phone's
//! decision back to that computer's own resolve route. The computer verifies the person and
//! signs the token with its kernel key, which never leaves the device. cloud-api only carries
//! the decision, and writes a receipt for each hop (`kernel_receipts`, see [`crate::kernel`]).
//!
//! | Route | Caller | What |
//! |---|---|---|
//! | `POST /api/v1/kernel/relay/requests` | paired runtime (runtime signature, as `runtime_events`) | upsert the relay row; first sight of a pending request pushes it |
//! | `GET /api/v1/kernel/relay/approvals` | Clerk session only | the owner's pending, unexpired requests as inbox rows with the `kernel` object |
//! | `POST /api/v1/kernel/relay/approvals/:runtime_id/:request_id/resolve` | Clerk session only | forward `{decision, note}` to the runtime, return its answer |
//!
//! **Who may resolve.** Only a person's Clerk session. A request that carries a run, agent,
//! internal-service, MCP-client or runtime credential is refused with `forbidden_caller` and a
//! `relay_refused` receipt before anything else is looked at; an API key is refused by
//! `require_session_user`. The forwarded request carries the phone's own Clerk token as both
//! `Authorization` and `x-allternit-human-proof`, so the device's own check
//! (`authority::routes::caller_and_proof`, `PersonProof::Session`) decides again, on the device.
//!
//! **Chats with no computer.** cloud-api's own approval requests (`platform_approvals` rows with
//! source `kernel`, [`crate::kernel_approvals`]) are listed here too, with `runtimeId` `"cloud"`,
//! and resolved on the same route with `:runtime_id` = `cloud`: after the same machine-credential
//! and session checks, cloud-api itself records the decision and, on approve, mints the token
//! (the action runs on cloud-api, so its kernel is the one that verifies it).
//!
//! **Expiry.** A row's `deadline` is the request's deadline. Past it the row reads as expired, is
//! marked `expired` by every list, and resolve answers 409 `token_expired` without forwarding (the
//! device rejects on expiry; `on_expiry` is always reject). A decision made on the device itself
//! is sent up with its new status and closes the row here.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::runtime_relay::{relay_request_to_runtime, RelayRequest};
use super::web_push::PushMessage;
use crate::kernel::{self, KernelKey, ReceiptIn};
use crate::ApiState;

pub const REQUESTS_PATH: &str = "/api/v1/kernel/relay/requests";
pub const APPROVALS_PATH: &str = "/api/v1/kernel/relay/approvals";
/// The device route the decision is forwarded to (allternit-api `authority::routes`).
pub const DEVICE_RESOLVE_PATH: &str = "/api/v1/kernel/approvals/resolve";
pub const PUSH_KIND: &str = "kernel_approval";
pub const PUSH_CATEGORY: &str = "KERNEL_APPROVAL";
const MAX_BODY_BYTES: usize = 32 * 1024;
const MAX_ANSWER_BYTES: usize = 256 * 1024;
const STATUSES: [&str; 5] = ["pending", "approved", "denied", "expired", "closed"];

/// Headers only machine callers send. Any of them on the phone route means it is not a person.
const MACHINE_HEADERS: [(&str, &str); 6] = [
    ("x-allternit-run-id", "run credential"),
    ("x-allternit-agent-id", "agent credential"),
    ("x-allternit-internal-token", "internal service credential"),
    ("x-allternit-mcp-client", "MCP client credential"),
    ("x-allternit-runtime-id", "runtime credential"),
    ("x-allternit-runtime-sig", "runtime credential"),
];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(REQUESTS_PATH, post(receive))
        .route(APPROVALS_PATH, get(list))
        .route("/api/v1/kernel/relay/approvals/:runtime_id/:request_id/resolve", post(resolve))
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": { "code": code, "message": message } }))).into_response()
}

fn clip(v: &Value, n: usize) -> String {
    v.as_str().unwrap_or("").trim().chars().take(n).collect()
}

/// The CONTRACT 6.4 `kernel` object, rebuilt from an allow-list (nothing else the device sends
/// reaches a phone).
fn kernel_object(k: &Value) -> Option<(String, DateTime<Utc>, Value)> {
    let request_id = clip(&k["requestId"], 128);
    let deadline = DateTime::parse_from_rfc3339(k["deadline"].as_str()?).ok()?.with_timezone(&Utc);
    if request_id.is_empty() {
        return None;
    }
    let obj = json!({
        "requestId": request_id, "runId": clip(&k["runId"], 128), "assignee": clip(&k["assignee"], 128),
        "actionClass": clip(&k["actionClass"], 32), "actionSummary": clip(&k["actionSummary"], 500),
        "actionHash": clip(&k["actionHash"], 80), "deadline": k["deadline"], "humanOnly": k["humanOnly"].as_bool().unwrap_or(true),
    });
    Some((request_id, deadline, obj))
}

#[derive(Deserialize)]
pub(crate) struct ReceiveBody {
    pub kernel: Value,
    #[serde(default, rename = "attentionId")]
    pub attention_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// The receipt principal for relay hops is the owner (their chain on cloud-api).
async fn receipt(db: &PgPool, owner: &str, run_id: &str, kind: &str, decision: &str, payload: Value) -> Result<String, String> {
    let key = KernelKey::from_env()?;
    kernel::append(
        db,
        &key,
        ReceiptIn { run_id, principal: owner, kind, schema_id: "allternit.os.ApprovalRelay", decision, payload, idempotency_key: None },
    )
    .await
    .map_err(|e| e.to_string())
}

/// The push for a new pending request: the same fan-out as other approvals (web push, APNs, FCM,
/// quiet hours honoured, needs-you level). The data names the runtime and request so the app
/// opens the resolve sheet; APPROVE needs device auth, DENY is destructive.
pub fn push_message(runtime_id: &str, kernel: &Value) -> PushMessage {
    let request_id = kernel["requestId"].as_str().unwrap_or("").to_string();
    let summary = kernel["actionSummary"].as_str().unwrap_or("").to_string();
    let data = json!({
        "kind": PUSH_KIND, "type": PUSH_KIND, "category": PUSH_CATEGORY,
        "actions": [ { "id": "APPROVE", "auth": true, "destructive": false }, { "id": "DENY", "auth": false, "destructive": true } ],
        "event": "kernel.approval.requested", "level": "needs_you",
        "runtimeId": runtime_id, "requestId": request_id, "kernel": kernel,
        "resolve": format!("{APPROVALS_PATH}/{runtime_id}/{request_id}/resolve"),
    });
    let title = if runtime_id == crate::kernel_approvals::CLOUD_RUNTIME { "Allternit needs your OK" } else { "Your computer needs your OK" };
    PushMessage {
        kind: PUSH_KIND,
        title: title.to_string(),
        body: summary,
        url: format!("/inbox?kernel={request_id}"),
        tag: format!("kernel-approval-{request_id}"),
        data,
        ttl_secs: 900,
        high_urgency: true,
    }
}

/// `POST /api/v1/kernel/relay/requests` from a paired runtime.
async fn receive(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "bad_request", "body too large");
    }
    let (owner, runtime_id) =
        match super::runtime_events::authenticate_runtime(&state.db, &headers, "POST", REQUESTS_PATH, &body, Utc::now().timestamp()).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    let Ok(req) = serde_json::from_slice::<ReceiveBody>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad_request", "invalid body");
    };
    match upsert(&state.db, &owner, &runtime_id, req).await {
        Ok(out) => {
            if let Some(msg) = out.push {
                super::aci_push::deliver(&state.db, &owner, &msg).await;
                let _ = sqlx::query("UPDATE kernel_approval_relay SET pushed_at = now() WHERE runtime_id = $1 AND request_id = $2")
                    .bind(&runtime_id)
                    .bind(&out.request_id)
                    .execute(&state.db)
                    .await;
            }
            (StatusCode::OK, Json(json!({ "ok": true, "requestId": out.request_id, "status": out.status }))).into_response()
        }
        Err(resp) => resp,
    }
}

pub(crate) struct Upserted {
    pub request_id: String,
    pub status: String,
    /// Set on first sight of a pending, unexpired request.
    pub push: Option<PushMessage>,
}

/// Store (or update) the relay row for an authenticated runtime. Pure of HTTP so tests drive it.
pub(crate) async fn upsert(db: &PgPool, owner: &str, runtime_id: &str, req: ReceiveBody) -> Result<Upserted, Response> {
    let Some((request_id, deadline, kernel_obj)) = kernel_object(&req.kernel) else {
        return Err(err(StatusCode::BAD_REQUEST, "bad_request", "kernel.requestId and an RFC 3339 kernel.deadline are required"));
    };
    let assignee = kernel_obj["assignee"].as_str().unwrap_or("");
    if assignee != "owner" && assignee != owner {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "bad_request", "only requests assigned to this computer's owner are relayed"));
    }
    let status = req.status.as_deref().unwrap_or("pending");
    if !STATUSES.contains(&status) {
        return Err(err(StatusCode::BAD_REQUEST, "bad_request", "unknown status"));
    }
    let attention = req.attention_id.map(|a| a.chars().take(128).collect::<String>());
    // A row already decided stays decided; a pending row takes the device's new status.
    let row: Result<(String, bool), sqlx::Error> = sqlx::query_as(
        "INSERT INTO kernel_approval_relay (request_id, runtime_id, owner_user_id, kernel, attention_id, status, deadline)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (runtime_id, request_id) DO UPDATE SET
             status = CASE WHEN kernel_approval_relay.status = 'pending' THEN EXCLUDED.status ELSE kernel_approval_relay.status END,
             resolved_at = CASE WHEN kernel_approval_relay.status = 'pending' AND EXCLUDED.status <> 'pending' THEN now() ELSE kernel_approval_relay.resolved_at END,
             updated_at = now()
         WHERE kernel_approval_relay.owner_user_id = EXCLUDED.owner_user_id
         RETURNING status, (xmax = 0) AS inserted",
    )
    .bind(&request_id)
    .bind(runtime_id)
    .bind(owner)
    .bind(&kernel_obj)
    .bind(&attention)
    .bind(status)
    .bind(deadline)
    .fetch_one(db)
    .await;
    let (stored, inserted) = match row {
        Ok(r) => r,
        Err(sqlx::Error::RowNotFound) => return Err(err(StatusCode::NOT_FOUND, "not_found", "request not found")),
        Err(e) => {
            tracing::error!("kernel relay: store failed: {e}");
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "relay store unavailable"));
        }
    };
    let run_id = kernel_obj["runId"].as_str().unwrap_or("").to_string();
    if let Err(e) = receipt(db, owner, &run_id, "relay_received", &stored, json!({ "request_id": request_id, "runtime_id": runtime_id, "device_status": status })).await {
        tracing::warn!("kernel relay: receipt not written ({e})");
    }
    let push = (inserted && stored == "pending" && deadline > Utc::now()).then(|| push_message(runtime_id, &kernel_obj));
    Ok(Upserted { request_id, status: stored, push })
}

/// Mark the owner's pending rows past their deadline as expired.
pub(crate) async fn expire_due(db: &PgPool, owner: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE kernel_approval_relay SET status = 'expired', updated_at = now()
         WHERE owner_user_id = $1 AND status = 'pending' AND deadline <= now()",
    )
    .bind(owner)
    .execute(db)
    .await?
    .rows_affected())
}

/// The owner's pending, unexpired requests as inbox rows (the #3303 shape plus where to resolve).
pub(crate) async fn pending_items(db: &PgPool, owner: &str) -> Result<Vec<Value>, sqlx::Error> {
    expire_due(db, owner).await?;
    let rows: Vec<(String, String, Value, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT request_id, runtime_id, kernel, attention_id, created_at FROM kernel_approval_relay
         WHERE owner_user_id = $1 AND status = 'pending' AND deadline > now()
         ORDER BY created_at DESC LIMIT 100",
    )
    .bind(owner)
    .fetch_all(db)
    .await?;
    let mut items: Vec<(DateTime<Utc>, Value)> = rows
        .into_iter()
        .map(|(request_id, runtime_id, k, attention_id, created_at)| {
            (
                created_at,
                json!({
                    "id": format!("kernel:{request_id}"), "kind": "kernel_approval", "source": "kernel",
                    "title": k["actionSummary"], "createdAt": created_at, "attentionId": attention_id,
                    "runtimeId": runtime_id, "actions": ["approve", "deny"],
                    "resolveRoute": format!("{APPROVALS_PATH}/{runtime_id}/{request_id}/resolve"),
                    "kernel": k,
                }),
            )
        })
        .collect();
    // cloud-api's own requests (chats with no computer), same shape, runtimeId "cloud".
    // A cloud store error never hides the device requests.
    let cloud = crate::kernel_approvals::pending_items(db, owner).await.unwrap_or_else(|e| {
        tracing::warn!("kernel relay: cloud requests not listed ({e})");
        Vec::new()
    });
    for item in cloud {
        let at = item["createdAt"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc)).unwrap_or_else(Utc::now);
        items.push((at, item));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items.truncate(100);
    Ok(items.into_iter().map(|(_, v)| v).collect())
}

/// `GET /api/v1/kernel/relay/approvals`: Clerk session only.
async fn list(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    if let Some(reason) = machine_marker(&headers) {
        return err(StatusCode::FORBIDDEN, "forbidden_caller", reason);
    }
    let owner = match super::connections::session_user(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match pending_items(&state.db, &owner).await {
        Ok(items) => (StatusCode::OK, Json(json!({ "ok": true, "items": items }))).into_response(),
        Err(e) => {
            tracing::error!("kernel relay: list failed: {e}");
            err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "relay store unavailable")
        }
    }
}

/// `Some(reason)` when the request carries a credential that is never a person's session.
pub(crate) fn machine_marker(headers: &HeaderMap) -> Option<&'static str> {
    MACHINE_HEADERS.iter().find(|(h, _)| headers.contains_key(*h)).map(|(_, why)| *why)
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhoneDecision {
    pub decision: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /api/v1/kernel/relay/approvals/:runtime_id/:request_id/resolve`.
async fn resolve(
    State(state): State<Arc<ApiState>>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(decision) = serde_json::from_slice::<PhoneDecision>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad_request", "body must be {decision, note?}");
    };
    resolve_for(&state, &runtime_id, &request_id, &headers, decision).await
}

/// The resolve flow, HTTP-free apart from the forward (tests drive it with a test relay).
pub(crate) async fn resolve_for(state: &ApiState, runtime_id: &str, request_id: &str, headers: &HeaderMap, body: PhoneDecision) -> Response {
    let db = &state.db;
    // 1. Never a machine credential, whatever else it carries. Receipt on the row owner's chain.
    let cloud = runtime_id == crate::kernel_approvals::CLOUD_RUNTIME;
    if let Some(reason) = machine_marker(headers) {
        if cloud {
            crate::kernel_approvals::refuse_receipt(db, request_id, reason).await;
            return err(StatusCode::FORBIDDEN, "forbidden_caller", "only a person's signed-in session can approve");
        }
        let owner: Option<String> =
            sqlx::query_scalar("SELECT owner_user_id FROM kernel_approval_relay WHERE runtime_id = $1 AND request_id = $2")
                .bind(runtime_id)
                .bind(request_id)
                .fetch_optional(db)
                .await
                .ok()
                .flatten();
        if let Some(owner) = owner {
            let _ = receipt(db, &owner, request_id, "relay_refused", "forbidden_caller", json!({ "request_id": request_id, "runtime_id": runtime_id, "reason": reason })).await;
        }
        return err(StatusCode::FORBIDDEN, "forbidden_caller", "only a person's signed-in session can approve");
    }
    if body.decision != "approve" && body.decision != "deny" {
        return err(StatusCode::BAD_REQUEST, "bad_request", "decision must be approve or deny");
    }
    // 2. A person's Clerk session (API keys are refused here).
    let user = match super::connections::session_user(state, headers).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    // cloud-api's own request: decided and (on approve) signed here.
    if cloud {
        return crate::kernel_approvals::resolve_cloud(db, &user, request_id, &body.decision, body.note.as_deref()).await;
    }
    // 3. The row, scoped to this person.
    let row: Option<(String, DateTime<Utc>, Value)> = match sqlx::query_as(
        "SELECT status, deadline, kernel FROM kernel_approval_relay WHERE runtime_id = $1 AND request_id = $2 AND owner_user_id = $3",
    )
    .bind(runtime_id)
    .bind(request_id)
    .bind(&user)
    .fetch_optional(db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("kernel relay: read failed: {e}");
            return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "relay store unavailable");
        }
    };
    let Some((status, deadline, k)) = row else { return err(StatusCode::NOT_FOUND, "not_found", "request not found") };
    let run_id = k["runId"].as_str().unwrap_or("").to_string();
    if deadline <= Utc::now() {
        let _ = expire_due(db, &user).await;
        return err(StatusCode::CONFLICT, "token_expired", "this request expired");
    }
    if status != "pending" {
        return err(StatusCode::CONFLICT, "conflict", "this request was already decided");
    }
    // 4. Write before act: no cloud receipt, no forward.
    let Some(token) = bearer(headers) else {
        return err(StatusCode::UNAUTHORIZED, "unauthenticated", "a signed-in session is required");
    };
    if let Err(e) = receipt(db, &user, &run_id, "relay_forwarded", &body.decision, json!({ "request_id": request_id, "runtime_id": runtime_id })).await {
        tracing::warn!("kernel relay: forward refused, receipt not written ({e})");
        return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "kernel receipt log unavailable");
    }
    // 5. Forward to the device. Its own check decides the person again; it signs the token.
    let forward = json!({ "request_id": request_id, "decision": body.decision, "note": body.note });
    let mut fwd_headers = HashMap::new();
    fwd_headers.insert("authorization".to_string(), format!("Bearer {token}"));
    fwd_headers.insert("x-allternit-human-proof".to_string(), token);
    fwd_headers.insert("content-type".to_string(), "application/json".to_string());
    let relayed = relay_request_to_runtime(
        db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &user,
        runtime_id,
        RelayRequest {
            method: "POST".into(),
            path: DEVICE_RESOLVE_PATH.into(),
            headers: fwd_headers,
            body: forward.to_string(),
            body_encoding: "utf8".into(),
        },
    )
    .await;
    let resp = match relayed {
        Ok(r) => r,
        Err(e) => {
            let _ = receipt(db, &user, &run_id, "relay_result", "unreachable", json!({ "request_id": request_id, "runtime_id": runtime_id })).await;
            tracing::info!("kernel relay: computer not reachable: {e}");
            return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "your computer is not reachable; try again when it is online");
        }
    };
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), MAX_ANSWER_BYTES).await.unwrap_or_default();
    let answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let ok = code.is_success() && answer["ok"].as_bool() == Some(true);
    // 6. Close the row from the device's answer, then the result receipt.
    let new_status = if ok {
        Some(if body.decision == "approve" { "approved" } else { "denied" })
    } else if code == StatusCode::CONFLICT {
        Some("closed")
    } else {
        None
    };
    if let Some(s) = new_status {
        let _ = sqlx::query(
            "UPDATE kernel_approval_relay SET status = $4, resolved_at = now(), resolved_by = $3, updated_at = now()
             WHERE runtime_id = $1 AND request_id = $2 AND status = 'pending'",
        )
        .bind(runtime_id)
        .bind(request_id)
        .bind(&user)
        .bind(s)
        .execute(db)
        .await;
    }
    let device_code = answer.pointer("/error/code").and_then(Value::as_str).unwrap_or(if ok { "ok" } else { "error" }).to_string();
    let _ = receipt(
        db,
        &user,
        &run_id,
        "relay_result",
        &device_code,
        json!({ "request_id": request_id, "runtime_id": runtime_id, "device_http": code.as_u16(), "device_receipt_id": answer.pointer("/error/receipt_id").cloned().unwrap_or(Value::Null) }),
    )
    .await;
    // The device's answer as is: the signed ApprovalTokenV1 on approve, the denied request on deny,
    // or the device's error (forbidden_caller, conflict, token_expired, …).
    // A relay-level answer (`runtime_offline`, a timeout) has no kernel envelope: say so plainly.
    if answer.get("ok").is_none() {
        return err(StatusCode::SERVICE_UNAVAILABLE, "kernel_unavailable", "no kernel answer from your computer; try again when it is online");
    }
    (code, Json(answer)).into_response()
}

#[cfg(test)]
#[path = "kernel_relay_tests.rs"]
mod tests;
