//! Code session status signals (phone calm step 9a, contract C6).
//!
//! Two flags live in a session's stored metadata bag (`session_metadata`) and
//! ride out on every session read (`transform_session` merges the bag):
//!
//! - `needsInput` (+ `needsInputAt`, `needsInputKind` = "permission" |
//!   "question"): the agent is waiting on the user. Set on `permission.asked`
//!   / `question.asked`; cleared when the last pending ask of that session is
//!   answered or dismissed, on `session.error`, when the run ends (idle) and
//!   when a new run starts.
//! - `readyForReview` (+ `readyForReviewAt`): the run finished having changed
//!   files or made an artifact, and nobody has opened the session since. The
//!   change signal is gizzi's `file.edited` bus event (published with the
//!   session id by the write, edit, apply_patch, notebook and workspace-edit
//!   tools) plus completed `artifact_create` / `artifact_update` tool parts.
//!   It is written when the session goes idle, cleared when a new run starts,
//!   and cleared by a client only through `PATCH {markReviewed: true}`.
//!
//! These keys are server-owned: `update_session` strips them from client
//! metadata so a stale full-bag write can neither set nor clear them.
//!
//! The watcher reads gizzi's `/v1/global/event` stream (every project
//! directory, not only the default instance) from a task spawned at startup,
//! so the flags update while no client is subscribed to `/agent-sessions/sync`
//! (the phone is closed). Each change is also broadcast as a `session_flags`
//! event on `/agent-sessions/sync` so open clients update without a refetch.
//!
//! Limits: events gizzi emits while the node is disconnected are missed (the
//! next idle or run start corrects `needsInput`); threads placed on another
//! Allternit (P4.2) get their flags from that server, not this one.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::http::HeaderMap;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::db::DbHandle;

pub const NEEDS_INPUT: &str = "needsInput";
pub const NEEDS_INPUT_AT: &str = "needsInputAt";
pub const NEEDS_INPUT_KIND: &str = "needsInputKind";
pub const READY_FOR_REVIEW: &str = "readyForReview";
pub const READY_FOR_REVIEW_AT: &str = "readyForReviewAt";

/// Metadata keys only the server writes. Cleared flags are removed from the
/// bag (absent = false).
pub const SERVER_OWNED_KEYS: [&str; 5] = [
    NEEDS_INPUT,
    NEEDS_INPUT_AT,
    NEEDS_INPUT_KIND,
    READY_FOR_REVIEW,
    READY_FOR_REVIEW_AT,
];

/// Tool parts whose completion counts as "this run made something to review"
/// (backup for `file.edited`, and the only signal for artifacts).
const CHANGE_TOOLS: [&str; 6] = [
    "artifact_create",
    "artifact_update",
    "write",
    "edit",
    "multiedit",
    "apply_patch",
];

/// Drop the server-owned keys from a client metadata bag.
pub fn strip_server_owned(bag: &mut Map<String, Value>) {
    for key in SERVER_OWNED_KEYS {
        bag.remove(key);
    }
}

/// One change the tracker asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagWrite {
    /// The agent asked the user something.
    NeedsInput { kind: &'static str },
    /// Nothing is waiting on the user any more.
    ClearNeedsInput,
    /// The run ended with changes: readyForReview on, needsInput off.
    ReadyForReview,
    /// A new run started: both flags off.
    RunStarted,
    /// The user opened the session (`markReviewed`).
    Reviewed,
}

#[derive(Debug, Default)]
struct SessionTrack {
    running: bool,
    changed: bool,
    /// Pending asks: request id -> "permission" | "question".
    pending: HashMap<String, &'static str>,
}

/// Per-session run state, in memory. Pure: feed it bus events, apply the
/// writes it returns.
#[derive(Debug, Default)]
pub struct Tracker {
    sessions: HashMap<String, SessionTrack>,
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

impl Tracker {
    pub fn apply(&mut self, event_type: &str, props: &Value) -> Option<(String, FlagWrite)> {
        match event_type {
            "session.status" => {
                let sid = str_at(props, "sessionID")?.to_string();
                let kind = props.get("status").and_then(|s| str_at(s, "type")).unwrap_or("idle");
                let track = self.sessions.entry(sid.clone()).or_default();
                if kind == "idle" {
                    let changed = std::mem::take(&mut track.changed);
                    track.running = false;
                    track.pending.clear();
                    let write = if changed { FlagWrite::ReadyForReview } else { FlagWrite::ClearNeedsInput };
                    Some((sid, write))
                } else if !track.running {
                    // busy / retry after idle (or the first status seen):
                    // a new run.
                    track.running = true;
                    track.changed = false;
                    track.pending.clear();
                    Some((sid, FlagWrite::RunStarted))
                } else {
                    None
                }
            }
            "permission.asked" | "question.asked" => {
                let sid = str_at(props, "sessionID")?.to_string();
                let kind = if event_type == "permission.asked" { "permission" } else { "question" };
                let track = self.sessions.entry(sid.clone()).or_default();
                if let Some(id) = str_at(props, "id") {
                    track.pending.insert(id.to_string(), kind);
                }
                Some((sid, FlagWrite::NeedsInput { kind }))
            }
            "permission.replied" | "question.replied" | "question.rejected" => {
                let sid = str_at(props, "sessionID")?.to_string();
                let track = self.sessions.entry(sid.clone()).or_default();
                if let Some(id) = str_at(props, "requestID") {
                    track.pending.remove(id);
                }
                match track.pending.values().next() {
                    None => Some((sid, FlagWrite::ClearNeedsInput)),
                    Some(kind) => Some((sid, FlagWrite::NeedsInput { kind: *kind })),
                }
            }
            "session.error" => {
                let sid = str_at(props, "sessionID")?.to_string();
                if let Some(track) = self.sessions.get_mut(&sid) {
                    track.pending.clear();
                }
                Some((sid, FlagWrite::ClearNeedsInput))
            }
            "file.edited" => {
                let sid = str_at(props, "sessionID")?;
                self.sessions.entry(sid.to_string()).or_default().changed = true;
                None
            }
            "message.part.updated" => {
                let part = props.get("part")?;
                if str_at(part, "type") != Some("tool") {
                    return None;
                }
                let tool = str_at(part, "tool")?;
                let done = part.get("state").and_then(|s| str_at(s, "status")) == Some("completed");
                if !done || !CHANGE_TOOLS.iter().any(|t| *t == tool) {
                    return None;
                }
                let sid = str_at(part, "sessionID").or_else(|| str_at(props, "sessionID"))?;
                self.sessions.entry(sid.to_string()).or_default().changed = true;
                None
            }
            "session.deleted" => {
                let info = props.get("info").unwrap_or(props);
                if let Some(id) = str_at(info, "id") {
                    self.sessions.remove(id);
                }
                None
            }
            _ => None,
        }
    }
}

/// Apply one write to a bag. Returns true when the bag changed.
pub fn apply_to_bag(bag: &mut Map<String, Value>, write: &FlagWrite, now: &str) -> bool {
    let before = bag.clone();
    match write {
        FlagWrite::NeedsInput { kind } => {
            if bag.get(NEEDS_INPUT) != Some(&Value::Bool(true)) {
                bag.insert(NEEDS_INPUT_AT.to_string(), json!(now));
            }
            bag.insert(NEEDS_INPUT.to_string(), json!(true));
            bag.insert(NEEDS_INPUT_KIND.to_string(), json!(kind));
        }
        FlagWrite::ClearNeedsInput => {
            bag.remove(NEEDS_INPUT);
            bag.remove(NEEDS_INPUT_AT);
            bag.remove(NEEDS_INPUT_KIND);
        }
        FlagWrite::ReadyForReview => {
            bag.remove(NEEDS_INPUT);
            bag.remove(NEEDS_INPUT_AT);
            bag.remove(NEEDS_INPUT_KIND);
            bag.insert(READY_FOR_REVIEW.to_string(), json!(true));
            bag.insert(READY_FOR_REVIEW_AT.to_string(), json!(now));
        }
        FlagWrite::RunStarted => {
            for key in SERVER_OWNED_KEYS {
                bag.remove(key);
            }
        }
        FlagWrite::Reviewed => {
            bag.remove(READY_FOR_REVIEW);
            bag.remove(READY_FOR_REVIEW_AT);
        }
    }
    *bag != before
}

/// The flag view clients get in a `session_flags` event: explicit values,
/// false / null when cleared.
pub fn flags_view(bag: &Map<String, Value>) -> Value {
    json!({
        NEEDS_INPUT: bag.get(NEEDS_INPUT).and_then(Value::as_bool).unwrap_or(false),
        NEEDS_INPUT_AT: bag.get(NEEDS_INPUT_AT).cloned().unwrap_or(Value::Null),
        NEEDS_INPUT_KIND: bag.get(NEEDS_INPUT_KIND).cloned().unwrap_or(Value::Null),
        READY_FOR_REVIEW: bag.get(READY_FOR_REVIEW).and_then(Value::as_bool).unwrap_or(false),
        READY_FOR_REVIEW_AT: bag.get(READY_FOR_REVIEW_AT).cloned().unwrap_or(Value::Null),
    })
}

/// Serializes read-modify-write of a session's metadata bag between the
/// watcher and `PATCH /agent-sessions/:id`, so neither loses the other's keys.
static BAG_LOCK: Mutex<()> = Mutex::new(());

/// Read the stored bag, let `f` change it, store it if `f` returns true.
/// Returns the bag after the change when it was stored.
pub fn update_bag(
    db: &DbHandle,
    session_id: &str,
    f: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Option<Map<String, Value>> {
    let _guard = BAG_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut bag = db
        .get_session_metadata(session_id)
        .ok()
        .flatten()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if !f(&mut bag) {
        return None;
    }
    db.set_session_metadata(session_id, &Value::Object(bag.clone())).ok()?;
    Some(bag)
}

static FLAGS_TX: OnceLock<broadcast::Sender<Value>> = OnceLock::new();

fn flags_tx() -> &'static broadcast::Sender<Value> {
    FLAGS_TX.get_or_init(|| broadcast::channel(256).0)
}

/// `session_flags` events for `/agent-sessions/sync` subscribers.
pub fn subscribe() -> broadcast::Receiver<Value> {
    flags_tx().subscribe()
}

/// Apply a write to the stored bag; broadcast it when something changed.
pub fn write_flags(db: &DbHandle, session_id: &str, write: &FlagWrite) -> bool {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    match update_bag(db, session_id, |bag| apply_to_bag(bag, write, &now)) {
        Some(bag) => {
            let _ = flags_tx().send(json!({
                "type": "session_flags",
                "session_id": session_id,
                "metadata": flags_view(&bag),
            }));
            true
        }
        None => false,
    }
}

/// Parse one SSE block from gizzi's global stream into `(type, properties)`.
/// Global frames are `{directory, payload: {type, properties}}`; a plain
/// `{type, properties}` frame is accepted too.
pub fn parse_global_frame(block: &str) -> Option<(String, Value)> {
    let data = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(&data).ok()?;
    let payload = value.get("payload").unwrap_or(&value);
    let event_type = payload.get("type").and_then(Value::as_str)?.to_string();
    let properties = payload.get("properties").cloned().unwrap_or(Value::Null);
    Some((event_type, properties))
}

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Start the always-on watcher. Reconnects forever with backoff.
pub fn spawn(db: DbHandle) {
    tokio::spawn(async move {
        let client = crate::agent_session_routes::gizzi_client(&HeaderMap::new());
        let base = crate::v1_routes::gizzi_base();
        let mut tracker = Tracker::default();
        let mut backoff = BACKOFF_INITIAL;
        loop {
            let mut opened = None;
            for path in ["/v1/global/event", "/global/event"] {
                match client
                    .get(format!("{base}{path}"))
                    .header("Accept", "text/event-stream")
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        opened = Some(resp);
                        break;
                    }
                    Ok(resp) => warn!(status = %resp.status(), path, "Code status watcher: subscribe refused"),
                    Err(err) => {
                        debug!(error = %err, "Code status watcher: gizzi unreachable");
                        break;
                    }
                }
            }
            if let Some(resp) = opened {
                info!("Code status watcher connected to gizzi");
                backoff = BACKOFF_INITIAL;
                let mut buf = String::new();
                let mut bytes = resp.bytes_stream();
                while let Some(chunk) = bytes.next().await {
                    let Ok(chunk) = chunk else { break };
                    buf.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(end) = buf.find("\n\n") {
                        let block = buf[..end].to_string();
                        buf.replace_range(..end + 2, "");
                        let Some((event_type, props)) = parse_global_frame(&block) else { continue };
                        if let Some((session_id, write)) = tracker.apply(&event_type, &props) {
                            let db = db.clone();
                            let _ = tokio::task::spawn_blocking(move || write_flags(&db, &session_id, &write)).await;
                        }
                    }
                }
                warn!("Code status watcher: gizzi stream closed; reconnecting");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(sid: &str, kind: &str) -> Value {
        json!({ "sessionID": sid, "status": { "type": kind } })
    }

    #[test]
    fn a_run_with_file_edits_ends_ready_for_review() {
        let mut t = Tracker::default();
        assert_eq!(t.apply("session.status", &status("s", "busy")), Some(("s".into(), FlagWrite::RunStarted)));
        assert_eq!(t.apply("session.status", &status("s", "busy")), None);
        assert_eq!(t.apply("file.edited", &json!({ "file": "/a.rs", "sessionID": "s" })), None);
        assert_eq!(t.apply("session.status", &status("s", "idle")), Some(("s".into(), FlagWrite::ReadyForReview)));
        // The next run starts clean.
        assert_eq!(t.apply("session.status", &status("s", "busy")), Some(("s".into(), FlagWrite::RunStarted)));
        assert_eq!(t.apply("session.status", &status("s", "idle")), Some(("s".into(), FlagWrite::ClearNeedsInput)));
    }

    #[test]
    fn a_completed_artifact_tool_counts_as_a_change() {
        let mut t = Tracker::default();
        t.apply("session.status", &status("s", "busy"));
        let running = json!({ "part": { "type": "tool", "tool": "artifact_create", "sessionID": "s", "state": { "status": "running" } } });
        t.apply("message.part.updated", &running);
        let read = json!({ "part": { "type": "tool", "tool": "read", "sessionID": "s", "state": { "status": "completed" } } });
        t.apply("message.part.updated", &read);
        assert_eq!(t.apply("session.status", &status("s", "idle")), Some(("s".into(), FlagWrite::ClearNeedsInput)));
        t.apply("session.status", &status("s", "busy"));
        let done = json!({ "part": { "type": "tool", "tool": "artifact_create", "sessionID": "s", "state": { "status": "completed" } } });
        t.apply("message.part.updated", &done);
        assert_eq!(t.apply("session.status", &status("s", "idle")), Some(("s".into(), FlagWrite::ReadyForReview)));
    }

    #[test]
    fn needs_input_clears_only_after_the_last_pending_ask() {
        let mut t = Tracker::default();
        t.apply("session.status", &status("s", "busy"));
        assert_eq!(
            t.apply("permission.asked", &json!({ "id": "p1", "sessionID": "s" })),
            Some(("s".into(), FlagWrite::NeedsInput { kind: "permission" }))
        );
        t.apply("question.asked", &json!({ "id": "q1", "sessionID": "s" }));
        assert_eq!(
            t.apply("permission.replied", &json!({ "requestID": "p1", "sessionID": "s" })),
            Some(("s".into(), FlagWrite::NeedsInput { kind: "question" }))
        );
        assert_eq!(
            t.apply("question.rejected", &json!({ "requestID": "q1", "sessionID": "s" })),
            Some(("s".into(), FlagWrite::ClearNeedsInput))
        );
        // A busy status mid-run does not clear a fresh ask.
        t.apply("question.asked", &json!({ "id": "q2", "sessionID": "s" }));
        assert_eq!(t.apply("session.status", &status("s", "busy")), None);
        assert_eq!(t.apply("session.error", &json!({ "sessionID": "s" })), Some(("s".into(), FlagWrite::ClearNeedsInput)));
    }

    #[test]
    fn bag_writes_and_the_flag_view() {
        let mut bag = Map::new();
        bag.insert("archived".into(), json!(true));
        assert!(apply_to_bag(&mut bag, &FlagWrite::NeedsInput { kind: "question" }, "t1"));
        assert_eq!(bag[NEEDS_INPUT_AT], json!("t1"));
        // Same ask again keeps the first time and changes nothing.
        assert!(!apply_to_bag(&mut bag, &FlagWrite::NeedsInput { kind: "question" }, "t2"));
        assert!(apply_to_bag(&mut bag, &FlagWrite::ReadyForReview, "t3"));
        let view = flags_view(&bag);
        assert_eq!(view[NEEDS_INPUT], json!(false));
        assert_eq!(view[READY_FOR_REVIEW], json!(true));
        assert!(apply_to_bag(&mut bag, &FlagWrite::Reviewed, "t4"));
        assert!(!apply_to_bag(&mut bag, &FlagWrite::RunStarted, "t5"));
        assert_eq!(bag.get("archived"), Some(&json!(true)));
    }

    #[test]
    fn client_metadata_loses_server_keys() {
        let mut bag = json!({ "archived": true, "needsInput": false, "readyForReview": true, "readyForReviewAt": "x" })
            .as_object()
            .cloned()
            .unwrap();
        strip_server_owned(&mut bag);
        assert_eq!(Value::Object(bag), json!({ "archived": true }));
    }

    #[test]
    fn global_frames_parse() {
        let block = "data: {\"directory\":\"/w\",\"payload\":{\"type\":\"file.edited\",\"properties\":{\"file\":\"a\",\"sessionID\":\"s\"}}}";
        let (kind, props) = parse_global_frame(block).unwrap();
        assert_eq!(kind, "file.edited");
        assert_eq!(props["sessionID"], "s");
        assert!(parse_global_frame(": comment").is_none());
    }
}
