//! Push when a live session (a pane running another CLI, or a gizzi session) becomes blocked:
//! the agent is waiting on a person. Runs off the heartbeat's session list, so it adds no
//! scheduler. One push per transition into `blocked`, keyed by the session address and its
//! `blockedSince` stamp; nothing while it stays blocked; a new stamp (it unblocked and blocked
//! again) pushes again. Uses the approvals push path (push worker `/notify`, paired device token)
//! and its opt-out: no push worker URL, or `ALLTERNIT_BLOCKED_PUSH=off`, means no push.
//!
//! The first pass after the API starts only records what is already blocked, so a restart does
//! not buzz the phone for sessions the person already knew about.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::channel_transports::{HttpReq, HttpSend};
use crate::factory_approvals_push::target_for;
use crate::AppState;

#[derive(Default)]
pub struct Watch {
    primed: bool,
    /// address -> the `blockedSince` already pushed (or seen on the priming pass).
    seen: HashMap<String, String>,
}

fn watch() -> &'static Mutex<Watch> {
    static W: OnceLock<Mutex<Watch>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(Watch::default()))
}

/// `ALLTERNIT_BLOCKED_PUSH=off|0|false|no` turns blocked-session push off.
pub fn opted_out() -> bool {
    std::env::var("ALLTERNIT_BLOCKED_PUSH").map(|v| matches!(v.trim().to_lowercase().as_str(), "off" | "0" | "false" | "no")).unwrap_or(false)
}

impl Watch {
    /// The sessions that just became blocked. Always updates what has been seen, even when the
    /// caller will not push (opted out), so turning push on later does not replay old blocks.
    pub fn observe(&mut self, sessions: &[Value]) -> Vec<Value> {
        let mut now_blocked: HashMap<String, String> = HashMap::new();
        let mut fresh = Vec::new();
        for s in sessions {
            if s.get("status").and_then(Value::as_str) != Some("blocked") {
                continue;
            }
            let Some(addr) = s.get("address").and_then(Value::as_str) else { continue };
            let since = s.get("blockedSince").and_then(Value::as_str).unwrap_or("").to_string();
            if self.primed && self.seen.get(addr) != Some(&since) {
                fresh.push(s.clone());
            }
            now_blocked.insert(addr.to_string(), since);
        }
        self.seen = now_blocked;
        self.primed = true;
        fresh
    }
}

/// The deep link that opens the session: its terminal thread when the pane is an Allternit
/// terminal, else the session row by address.
pub fn open_url(s: &Value) -> String {
    match s.get("terminalId").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        Some(t) => format!("/terminal/{t}"),
        None => format!("/factory/sessions/{}", s.get("address").and_then(Value::as_str).unwrap_or("")),
    }
}

pub fn notify_body(s: &Value, runtime_id: &str) -> Value {
    let name = s.get("name").and_then(Value::as_str).unwrap_or("A session");
    let body = s.get("doing").and_then(Value::as_str).filter(|d| !d.trim().is_empty()).unwrap_or("Waiting for your answer");
    let addr = s.get("address").and_then(Value::as_str).unwrap_or("");
    let since = s.get("blockedSince").and_then(Value::as_str).unwrap_or("");
    json!({
        "runtimeId": runtime_id,
        "title": format!("{name} needs you"),
        "body": body,
        "tag": format!("live-session-blocked:{addr}"),
        "type": "permission",
        "data": {
            "kind": "live-session.blocked",
            "address": addr,
            "sessionId": s.get("sessionId"),
            "terminalId": s.get("terminalId"),
            "blockedSince": since,
            "openUrl": open_url(s),
        },
        "actions": [ { "action": "open", "title": "Open" } ],
    })
}

/// Called with each heartbeat's session list. Never fails the heartbeat.
pub async fn notify_transitions(state: &AppState, owner: &str, sessions: &Value) {
    let list = sessions.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let fresh = watch().lock().unwrap_or_else(|e| e.into_inner()).observe(list);
    if fresh.is_empty() || opted_out() {
        return;
    }
    let Ok(t) = target_for(state, owner) else { return };
    let http: Arc<dyn HttpSend> = crate::factory_approvals_channels::http();
    for s in &fresh {
        if let Err(why) = send(&t.worker, &t.token, &t.runtime_id, s, http.clone()).await {
            tracing::info!(session = ?s.get("address"), why, "blocked session: no push");
        }
    }
}

async fn send(worker: &str, token: &str, runtime_id: &str, s: &Value, http: Arc<dyn HttpSend>) -> Result<(), String> {
    let resp = http
        .post_json(HttpReq {
            url: format!("{}/notify", worker.trim_end_matches('/')),
            headers: vec![("authorization".into(), format!("Bearer {token}"))],
            body: notify_body(s, runtime_id),
        })
        .await?;
    if (200..300).contains(&resp.status) { Ok(()) } else { Err(format!("push worker returned {}", resp.status)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(addr: &str, status: &str, since: Option<&str>) -> Value {
        let mut v = json!({ "address": addr, "name": "api-codex", "status": status, "doing": "Run cargo test?", "terminalId": "term_1" });
        if let Some(s) = since {
            v["blockedSince"] = json!(s);
        }
        v
    }

    #[test]
    fn one_push_per_blocked_transition() {
        let mut w = Watch::default();
        assert!(w.observe(&[row("me/a", "busy", None)]).is_empty(), "priming pass");
        assert_eq!(w.observe(&[row("me/a", "blocked", Some("t1"))]).len(), 1, "busy -> blocked pushes");
        assert!(w.observe(&[row("me/a", "blocked", Some("t1"))]).is_empty(), "nothing while it stays blocked");
        assert!(w.observe(&[row("me/a", "blocked", Some("t1"))]).is_empty());
        assert!(w.observe(&[row("me/a", "busy", None)]).is_empty());
        assert_eq!(w.observe(&[row("me/a", "blocked", Some("t2"))]).len(), 1, "blocked again pushes again");
    }

    #[test]
    fn already_blocked_at_startup_does_not_push() {
        let mut w = Watch::default();
        assert!(w.observe(&[row("me/a", "blocked", Some("t0"))]).is_empty());
        assert!(w.observe(&[row("me/a", "blocked", Some("t0"))]).is_empty());
    }

    #[test]
    fn opt_out_is_read_from_the_environment() {
        std::env::set_var("ALLTERNIT_BLOCKED_PUSH", "off");
        assert!(opted_out());
        std::env::remove_var("ALLTERNIT_BLOCKED_PUSH");
        assert!(!opted_out());
    }

    #[test]
    fn body_and_deep_link() {
        let b = notify_body(&row("me/a", "blocked", Some("t1")), "rt_1");
        assert_eq!(b["title"], "api-codex needs you");
        assert_eq!(b["body"], "Run cargo test?");
        assert_eq!(b["data"]["openUrl"], "/terminal/term_1");
        let mut no_term = row("me/b", "blocked", Some("t1"));
        no_term.as_object_mut().unwrap().remove("terminalId");
        assert_eq!(open_url(&no_term), "/factory/sessions/me/b");
    }
}
