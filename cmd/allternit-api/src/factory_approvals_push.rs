//! Push for Factory approvals: a Web Push through the Remote Control push
//! worker (`services/remote-control-push`, `POST /notify`), authenticated as
//! this runtime with its device token. Push counts as "set up" only when the
//! push worker URL is configured, this runtime is paired, and it is paired to
//! the approval's owner, so a notification never reaches another account.
//!
//! The notification carries a one-time push code. Approve on the notification
//! calls `POST /api/factory/approvals/:id/push-action` with `{ decision, code }`
//! from the device's own signed-in session, so it needs both the owner's
//! session and the code.

use std::sync::Arc;

use chrono::Utc;
use serde_json::{json, Value};

use crate::channel_transports::{HttpReq, HttpSend};
use crate::factory_approvals::{issue_code, Approval};
use crate::AppState;

/// Which runtime this API serves: `ALLTERNIT_RUNTIME_ID`, else `runtimeId`
/// in the shared runtime identity file (written at pairing).
pub fn runtime_id() -> Option<String> {
    if let Some(id) = std::env::var("ALLTERNIT_RUNTIME_ID").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Some(id);
    }
    let path = std::env::var("ALLTERNIT_RUNTIME_IDENTITY_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config").join("allternit").join("runtime-identity.json")))?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&raw).ok()?.get("runtimeId").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

/// What a push needs; `Err(why)` when push isn't set up for this owner.
pub struct PushTarget {
    pub worker: String,
    pub token: String,
    pub runtime_id: String,
}

pub fn target_for(state: &AppState, owner: &str) -> Result<PushTarget, String> {
    let worker = state.config.push_worker_url().ok_or("the push worker URL is not configured")?;
    let secret = crate::relay_auth::process_secret();
    let token = secret.device_token().ok_or("this runtime isn't paired")?;
    if secret.paired_owner().as_deref() != Some(owner) {
        return Err("this runtime is paired to another account".into());
    }
    let runtime_id = runtime_id().ok_or("this runtime has no runtime id")?;
    Ok(PushTarget { worker, token, runtime_id })
}

/// The request body for the push worker (`/notify`). `data` and `actions`
/// carry the approval for the service worker's Approve / Open buttons.
pub fn notify_body(a: &Approval, runtime_id: &str, code: &str) -> Value {
    let body = if a.is_high_risk() { format!("{} (high risk: app or push only)", a.summary) } else { a.summary.clone() };
    json!({
        "runtimeId": runtime_id,
        "title": format!("Approve: {}", a.title),
        "body": body,
        "tag": format!("factory-approval:{}", a.id),
        "type": "permission",
        "data": {
            "kind": "factory.approval",
            "approvalId": a.id,
            "dagId": a.dag_id,
            "nodeId": a.node_id,
            "code": code,
            "actionUrl": format!("/api/factory/approvals/{}/push-action", a.id),
            "openUrl": format!("/factory/nodes/{}/{}", a.dag_id, a.node_id),
        },
        "actions": [ { "action": "approve", "title": "Approve" }, { "action": "open", "title": "Open" } ],
    })
}

pub async fn send_request(state: &Arc<AppState>, a: &Approval) -> Result<(), String> {
    send_request_with(state, a, crate::factory_approvals_channels::http()).await
}

pub async fn send_request_with(state: &Arc<AppState>, a: &Approval, http: Arc<dyn HttpSend>) -> Result<(), String> {
    let t = target_for(state, &a.owner)?;
    let code = {
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        issue_code(&conn, &a.id, "push", None, Utc::now()).map_err(|e| e.to_string())?
    };
    let resp = http
        .post_json(HttpReq {
            url: format!("{}/notify", t.worker.trim_end_matches('/')),
            headers: vec![("authorization".into(), format!("Bearer {}", t.token))],
            body: notify_body(a, &t.runtime_id, &code),
        })
        .await?;
    if !(200..300).contains(&resp.status) {
        return Err(format!("push worker returned {}", resp.status));
    }
    match resp.body.get("delivered").and_then(Value::as_i64) {
        Some(0) => Err("no device is subscribed to push for this runtime".into()),
        _ => Ok(()),
    }
}
