//! Send kernel approval requests for this computer's owner up to cloud-api, so they reach the
//! owner's phones (AllternitOS Stage 1, plan step 18; CONTRACT.md 6.4).
//!
//! The channel is the one every runtime → cloud call uses: [`crate::account_sync::SignedAccountCloud`]
//! (`ALLTERNIT_CLOUD_API_URL`, signed with this runtime's device token; cloud-api checks it with
//! `runtime_events::authenticate_runtime`). Body: `{kernel, attentionId, status}` where `kernel` is
//! the inbox row's CONTRACT 6.4 object. Never the action payload, never a token.
//!
//! The way back: the phone's decision reaches this computer through cloud-api's runtime relay as
//! a normal `POST /api/v1/kernel/approvals/resolve` carrying the phone's Clerk session in
//! `x-allternit-human-proof`. [`super::routes`] decides the person again here, and the kernel key
//! that signs the token never leaves this computer.
//!
//! Best effort and off the request path: an unpaired computer (no device token) or an
//! unreachable cloud-api only logs. The request still shows on Desktop and in the inbox here.

use allternit_authority::{ApprovalRequestV1, ApprovalStatus};
use serde_json::{json, Value};

use crate::account_sync::{AccountCloud, SignedAccountCloud};

/// cloud-api route (`routes::kernel_relay`).
pub const RELAY_PATH: &str = "/api/v1/kernel/relay/requests";

/// True when the request is for this computer's owner (the only requests relayed to phones).
pub fn for_owner(req: &ApprovalRequestV1, owner: &str) -> bool {
    req.assignee == "owner" || req.assignee == owner
}

fn enum_str<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// The relay body for one request: the same `kernel` object as the inbox row.
pub fn body(req: &ApprovalRequestV1) -> Value {
    let status = match req.status {
        None | Some(ApprovalStatus::Open) => "pending",
        Some(ApprovalStatus::Approved) => "approved",
        Some(ApprovalStatus::Denied) => "denied",
        Some(ApprovalStatus::Expired) => "expired",
        Some(ApprovalStatus::Withdrawn) => "closed",
    };
    json!({
        "kernel": {
            "requestId": req.request_id, "runId": req.run_id, "assignee": req.assignee,
            "actionClass": enum_str(req.action_class), "actionSummary": req.action_summary,
            "actionHash": req.action_hash, "deadline": req.deadline,
            "humanOnly": allternit_authority::policy::is_human_only(req.action_class),
        },
        "attentionId": req.attention_id,
        "status": status,
    })
}

/// A decision made here (Desktop, the TUI, or the phone through the relay) closes the phone's
/// copy. Approve returns only the token, so the body names the request and its new status; cloud-api
/// never reopens a decided row and only updates the status of an existing one.
pub fn closed_body(request_id: &str, status: &str, deadline: &str) -> Value {
    json!({ "kernel": { "requestId": request_id, "assignee": "owner", "deadline": deadline }, "status": status })
}

fn send(handle: &tokio::runtime::Handle, payload: Vec<u8>) {
    handle.spawn(async move {
        match SignedAccountCloud.send("POST", RELAY_PATH, payload).await {
            Ok((status, _)) if (200..300).contains(&status) => {}
            Ok((status, answer)) => {
                tracing::warn!(status, answer = %String::from_utf8_lossy(&answer).chars().take(200).collect::<String>(), "kernel relay: cloud-api refused");
            }
            Err(e) => tracing::debug!("kernel relay: not sent ({e})"),
        }
    });
}

/// Tell cloud-api a request was decided here. Best effort, like [`notify`].
pub fn notify_closed(handle: &tokio::runtime::Handle, request_id: &str, status: &str, deadline: &str) {
    send(handle, closed_body(request_id, status, deadline).to_string().into_bytes());
}

/// Send `req` to cloud-api on the current runtime, without waiting. No-op for a request that is
/// not for `owner`.
pub fn notify(handle: &tokio::runtime::Handle, req: &ApprovalRequestV1, owner: &str) {
    if !for_owner(req, owner) {
        return;
    }
    send(handle, body(req).to_string().into_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(assignee: &str) -> ApprovalRequestV1 {
        serde_json::from_value(json!({
            "schema_id": "allternit.os.ApprovalRequestV1", "schema_version": "1.0.0", "request_id": "apr_1",
            "source_store": "kernel", "run_id": "run_1", "principal": "agent_1", "assignee": assignee,
            "action_class": "money", "action_summary": "Send invoice", "action_payload": { "amount": 4000, "card": "4242" },
            "action_hash": "sha256:abc", "deadline": "2026-11-28T12:00:00Z", "on_expiry": "reject", "check_id": "chk_1",
        }))
        .unwrap()
    }

    #[test]
    fn only_owner_requests_are_relayed_and_the_payload_never_leaves() {
        assert!(for_owner(&req("owner"), "user_a"));
        assert!(for_owner(&req("user_a"), "user_a"));
        assert!(!for_owner(&req("user_b"), "user_a"));
        let b = body(&req("owner"));
        assert_eq!(b["status"], "pending");
        assert_eq!(b["kernel"]["humanOnly"], true);
        assert_eq!(b["kernel"]["actionClass"], "money");
        assert!(!b.to_string().contains("4242"), "the action payload is not relayed");
    }
}
