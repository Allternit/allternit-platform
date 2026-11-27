//! Agent-browser pushes: `POST /push/notify/:runtime_id`, the receiver for what allternit-api
//! forwards (`internal_routes.rs` `notify_push`) when a browser run needs the person.
//!
//! Three kinds, each a short title and body plus data the app opens the run with:
//!
//! * `aci_approval`: `{kind, run_id, computer_id, approval_id, approval_kind: "action"|"credential",
//!   credential?: {name, domain}, message}`. APNs category `ACI_APPROVAL` (APPROVE needs auth, DENY is destructive).
//! * `aci_human`: `{kind, run_id, computer_id, reason, secret}`. Category `ACI_HUMAN` (CONTINUE, STOP).
//! * `aci_blocked`: `{kind, run_id, computer_id, domain, block_kind, rerun_targets}`. Category `ACI_BLOCKED` (OPEN).
//!
//! The data is rebuilt from an allowlist, so a credential value (`value`, `password`, a token, anything
//! unlisted) never reaches a device even when the runtime sends one. Web push and FCM carry the same
//! action ids in `data.actions`. Auth is the runtime relay signature (see `runtime_events`), and the
//! owner is the runtime's paired user.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::Utc;
use serde_json::{json, Value};

use super::notifications::{self, Level};
use super::web_push::{self, PushMessage, Vapid};
use crate::ApiState;
use super::{apns, fcm};

pub const EVENT: &str = "aci.attention";
pub const MAX_BODY_BYTES: usize = 16 * 1024;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/push/notify/:runtime_id", post(notify_route))
}

fn clip(v: Option<&Value>, n: usize) -> String {
    v.and_then(Value::as_str).unwrap_or("").trim().chars().take(n).collect()
}

fn one_of(v: Option<&Value>, allowed: &[&str], default: &str) -> String {
    let s = v.and_then(Value::as_str).unwrap_or("");
    if allowed.contains(&s) { s.to_string() } else { default.to_string() }
}

/// Category id and action list for a kind. `auth` = the action needs device authentication,
/// `destructive` = it is shown as destructive and needs none.
pub fn category(kind: &str) -> Option<(&'static str, Value)> {
    match kind {
        "aci_approval" => Some(("ACI_APPROVAL", json!([
            { "id": "APPROVE", "auth": true, "destructive": false },
            { "id": "DENY", "auth": false, "destructive": true },
        ]))),
        "aci_human" => Some(("ACI_HUMAN", json!([
            { "id": "CONTINUE", "auth": false, "destructive": false },
            { "id": "STOP", "auth": false, "destructive": false },
        ]))),
        "aci_blocked" => Some(("ACI_BLOCKED", json!([{ "id": "OPEN", "auth": false, "destructive": false }]))),
        _ => None,
    }
}

fn static_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "aci_approval" => Some("aci_approval"),
        "aci_human" => Some("aci_human"),
        "aci_blocked" => Some("aci_blocked"),
        _ => None,
    }
}

/// Build the push for `raw` (the request's `data`; `bot` names the bot). `None` for an unknown kind or a
/// payload without a run and computer. Only allowlisted fields are copied.
pub fn build(raw: &Value, bot: &str) -> Option<PushMessage> {
    let kind_str = raw.get("kind").and_then(Value::as_str)?;
    let kind = static_kind(kind_str)?;
    let (cat, actions) = category(kind)?;
    let run_id = clip(raw.get("run_id"), 128);
    let computer_id = clip(raw.get("computer_id"), 128);
    if run_id.is_empty() || computer_id.is_empty() {
        return None;
    }
    let bot = { let b = bot.trim(); if b.is_empty() { "Your bot".to_string() } else { b.chars().take(60).collect() } };
    let (title, body, mut data, tag) = match kind {
        "aci_approval" => {
            let approval_id = clip(raw.get("approval_id"), 128);
            if approval_id.is_empty() {
                return None;
            }
            let approval_kind = one_of(raw.get("approval_kind"), &["action", "credential"], "action");
            let message = clip(raw.get("message"), 300);
            let mut data = json!({ "kind": kind, "run_id": run_id, "computer_id": computer_id, "approval_id": approval_id, "approval_kind": approval_kind, "message": message });
            let mut body = message;
            if approval_kind == "credential" {
                let (name, domain) = (clip(raw.pointer("/credential/name"), 80), clip(raw.pointer("/credential/domain"), 120));
                if !name.is_empty() || !domain.is_empty() {
                    body = format!("Fill your {name} login on {domain}?");
                    data["credential"] = json!({ "name": name, "domain": domain });
                }
            }
            (format!("{bot} needs your OK"), body, data, format!("aci-approval-{approval_id}"))
        }
        "aci_human" => {
            let reason = clip(raw.get("reason"), 300);
            let secret = raw.get("secret").and_then(Value::as_bool).unwrap_or(false);
            (
                format!("{bot} needs you"),
                reason.clone(),
                json!({ "kind": kind, "run_id": run_id, "computer_id": computer_id, "reason": reason, "secret": secret }),
                format!("aci-human-{run_id}"),
            )
        }
        _ => {
            let domain = clip(raw.get("domain"), 120);
            let block_kind = one_of(raw.get("block_kind"), &["login", "two_factor", "captcha", "block_page", "rate_limited"], "block_page");
            let targets: Vec<Value> = raw.get("rerun_targets").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).take(10).map(|s| json!(s.chars().take(128).collect::<String>())).collect()).unwrap_or_default();
            (
                format!("{domain} blocked {bot}"),
                "Open to continue, stop or run it on your Mac".to_string(),
                json!({ "kind": kind, "run_id": run_id, "computer_id": computer_id, "domain": domain, "block_kind": block_kind, "rerun_targets": targets }),
                format!("aci-blocked-{run_id}"),
            )
        }
    };
    data["type"] = json!(kind);
    data["category"] = json!(cat);
    data["actions"] = actions;
    data["event"] = json!(EVENT);
    data["level"] = json!("needs_you");
    data["computerId"] = json!(computer_id);
    Some(PushMessage { kind, title, body, url: format!("/computers/{computer_id}?run={run_id}"), tag, data, ttl_secs: 3600, high_urgency: true })
}

/// Send `msg` to `user` through web push, APNs and FCM (the same fan-out as computer alerts). Quiet hours hold it
/// unless it is urgent; these always need the person, so they follow the `needs_you` level.
pub async fn deliver(db: &sqlx::PgPool, user: &str, msg: &PushMessage) {
    if notifications::hold_if_quiet(db, user, EVENT, msg, Level::NeedsYou, None, None).await {
        return;
    }
    match Vapid::from_env() {
        Some(v) => {
            web_push::send_to_user(db, web_push::default_transport().as_ref(), &v, user, msg).await;
        }
        None => {
            apns::send_to_user(db, user, msg).await;
            fcm::send_to_user(db, user, msg).await;
        }
    }
}

fn err(status: StatusCode, e: &str) -> Response {
    (status, Json(json!({ "error": e }))).into_response()
}

async fn notify_route(State(state): State<Arc<ApiState>>, Path(runtime_id): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large");
    }
    let path = format!("/push/notify/{runtime_id}");
    let (user, signed) = match super::runtime_events::authenticate_runtime(&state.db, &headers, "POST", &path, &body, Utc::now().timestamp()).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if signed != runtime_id {
        return err(StatusCode::UNAUTHORIZED, "invalid_runtime");
    }
    let Ok(req) = serde_json::from_slice::<Value>(&body) else { return err(StatusCode::BAD_REQUEST, "invalid_body") };
    let data = req.get("data").unwrap_or(&Value::Null);
    let bot = req.pointer("/data/bot_name").or_else(|| req.get("bot_name")).and_then(Value::as_str).unwrap_or("");
    let Some(msg) = build(data, bot) else { return err(StatusCode::UNPROCESSABLE_ENTITY, "unknown_kind") };
    deliver(&state.db, &user, &msg).await;
    Json(json!({ "ok": true, "kind": msg.kind })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_maps_to_its_category_and_actions() {
        let a = build(&json!({ "kind": "aci_approval", "run_id": "r", "computer_id": "c", "approval_id": "a1", "approval_kind": "action", "message": "Send the form?" }), "Scout").unwrap();
        assert_eq!((a.kind, a.data["category"].as_str(), a.title.as_str(), a.body.as_str()), ("aci_approval", Some("ACI_APPROVAL"), "Scout needs your OK", "Send the form?"));
        assert_eq!(a.data["actions"], json!([{ "id": "APPROVE", "auth": true, "destructive": false }, { "id": "DENY", "auth": false, "destructive": true }]));
        let h = build(&json!({ "kind": "aci_human", "run_id": "r", "computer_id": "c", "reason": "Enter the code", "secret": true }), "Scout").unwrap();
        assert_eq!((h.data["category"].as_str(), h.title.as_str(), h.body.as_str(), h.data["secret"].as_bool()), (Some("ACI_HUMAN"), "Scout needs you", "Enter the code", Some(true)));
        let ids: Vec<&str> = h.data["actions"].as_array().unwrap().iter().filter_map(|x| x["id"].as_str()).collect();
        assert_eq!(ids, ["CONTINUE", "STOP"]);
        let b = build(&json!({ "kind": "aci_blocked", "run_id": "r", "computer_id": "c", "domain": "bank.example", "block_kind": "captcha", "rerun_targets": ["mac-1"] }), "Scout").unwrap();
        assert_eq!((b.data["category"].as_str(), b.title.as_str(), b.data["block_kind"].as_str()), (Some("ACI_BLOCKED"), "bank.example blocked Scout", Some("captcha")));
        assert_eq!(b.data["actions"], json!([{ "id": "OPEN", "auth": false, "destructive": false }]));
        assert!(build(&json!({ "kind": "message", "run_id": "r", "computer_id": "c" }), "Scout").is_none());
    }

    #[test]
    fn a_credential_approval_names_the_login_and_never_carries_a_value() {
        let raw = json!({
            "kind": "aci_approval", "run_id": "r", "computer_id": "c", "approval_id": "a1", "approval_kind": "credential",
            "credential": { "name": "Acme", "domain": "acme.test", "value": "hunter2-secret", "password": "hunter2-secret" },
            "value": "hunter2-secret", "password": "hunter2-secret", "message": "Fill it?"
        });
        let m = build(&raw, "Scout").unwrap();
        assert_eq!(m.body, "Fill your Acme login on acme.test?");
        assert_eq!(m.data["credential"], json!({ "name": "Acme", "domain": "acme.test" }));
        let all = serde_json::to_string(&m.data).unwrap() + &m.title + &m.body;
        assert!(!all.contains("hunter2-secret") && m.data.get("value").is_none() && m.data.get("password").is_none());
        // The FCM and APNs bodies are built from this same data.
        assert!(!fcm::message(&m, "tok").to_string().contains("hunter2-secret"));
    }
}
