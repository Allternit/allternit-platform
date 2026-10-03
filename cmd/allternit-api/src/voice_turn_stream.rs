//! The streaming bot turn behind `POST /api/v1/voice/calls/{id}/turn`.
//!
//! `send_bot_turn` (the channel path) reports only the final reply, so a caller
//! waited for the whole answer before hearing anything. A native gizzi turn is
//! streamed here instead: subscribe to gizzi's `/event` stream, send the same
//! message the channel path sends, and turn the session's bus events into the
//! call stream's `text.delta` and `tool` events as they happen.
//!
//! Vendor-bound and placed (remote) sessions have no local event stream; the
//! caller falls back to the final-reply behaviour for those (see
//! `voice_calls::GizziStreamTurner`), with a log line.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

use crate::voice_calls::TurnReply;

/// A turn that has not finished by now is cut off (the caller is on a phone).
const TURN_DEADLINE: Duration = Duration::from_secs(120);
/// After the message call returns, wait this long for trailing bus events.
const TRAILING_EVENTS: Duration = Duration::from_millis(250);

/// What the bot says when a tool wants an approval: nobody can click on a call.
pub(crate) const APPROVAL_SPOKEN: &str = "I need your approval to do that, and I can't ask for it on a call. Please approve it in the app, or ask me to do something else.";

#[derive(Clone, Copy, PartialEq)]
enum PartKind {
    Text,
    Other,
}

pub(crate) enum Flow {
    Continue,
    /// The session went idle.
    Idle,
    /// A tool asked for an approval that cannot be answered here.
    Approval,
    Failed(String),
}

/// Folds one session's gizzi bus events into call-stream events.
pub(crate) struct TurnFold {
    session_id: String,
    parts: HashMap<String, PartKind>,
    part_message: HashMap<String, String>,
    user_messages: HashSet<String>,
    /// Deltas that arrived before their part declared its type, in order.
    pending: Vec<(String, String)>,
    buf: String,
    /// callID -> 1 started, 2 finished.
    tools: HashMap<String, u8>,
    pub(crate) spoke: bool,
}

impl TurnFold {
    pub(crate) fn new(session_id: &str) -> Self {
        Self { session_id: session_id.into(), parts: HashMap::new(), part_message: HashMap::new(), user_messages: HashSet::new(), pending: Vec::new(), buf: String::new(), tools: HashMap::new(), spoke: false }
    }

    fn say(&mut self, out: &mut Vec<Value>, text: String) {
        if !text.is_empty() {
            self.spoke = true;
            out.push(json!({ "type": "text.delta", "text": text }));
        }
    }

    /// Emit whole words only; the unfinished last word waits for its end.
    fn push_text(&mut self, out: &mut Vec<Value>, delta: &str) {
        self.buf.push_str(delta);
        if let Some((i, c)) = self.buf.char_indices().rev().find(|(_, c)| c.is_whitespace()) {
            let rest = self.buf.split_off(i + c.len_utf8());
            let head = std::mem::replace(&mut self.buf, rest);
            self.say(out, head);
        }
    }

    pub(crate) fn flush(&mut self, out: &mut Vec<Value>) {
        let head = std::mem::take(&mut self.buf);
        self.say(out, head);
    }

    fn kind_of(&self, part_id: &str) -> Option<PartKind> {
        let kind = *self.parts.get(part_id)?;
        let from_user = self.part_message.get(part_id).is_some_and(|m| self.user_messages.contains(m));
        Some(if from_user { PartKind::Other } else { kind })
    }

    fn release_pending(&mut self, out: &mut Vec<Value>) {
        let pending = std::mem::take(&mut self.pending);
        for (part, delta) in pending {
            match self.kind_of(&part) {
                Some(PartKind::Text) => self.push_text(out, &delta),
                Some(PartKind::Other) => {}
                None => self.pending.push((part, delta)),
            }
        }
    }

    pub(crate) fn feed(&mut self, ev: &Value, out: &mut Vec<Value>) -> Flow {
        let props = &ev["properties"];
        let session = props["sessionID"].as_str().or_else(|| props["part"]["sessionID"].as_str()).or_else(|| props["info"]["sessionID"].as_str());
        if session != Some(self.session_id.as_str()) {
            return Flow::Continue;
        }
        match ev["type"].as_str().unwrap_or("") {
            "message.updated" => {
                if props["info"]["role"] == "user" {
                    if let Some(id) = props["info"]["id"].as_str() {
                        self.user_messages.insert(id.to_string());
                        self.release_pending(out);
                    }
                }
            }
            "message.part.delta" => {
                if props["field"].as_str().unwrap_or("text") != "text" {
                    return Flow::Continue;
                }
                let (part, delta) = (props["partID"].as_str().unwrap_or(""), props["delta"].as_str().unwrap_or(""));
                if delta.is_empty() {
                    return Flow::Continue;
                }
                match self.kind_of(part) {
                    Some(PartKind::Text) => self.push_text(out, delta),
                    Some(PartKind::Other) => {}
                    None => self.pending.push((part.to_string(), delta.to_string())),
                }
            }
            "message.part.updated" => {
                let part = &props["part"];
                let id = part["id"].as_str().unwrap_or("");
                match part["type"].as_str().unwrap_or("") {
                    "tool" | "tool_use" => self.tool(part, out),
                    kind => {
                        if !id.is_empty() {
                            self.parts.insert(id.to_string(), if kind == "text" { PartKind::Text } else { PartKind::Other });
                            if let Some(m) = part["messageID"].as_str() {
                                self.part_message.insert(id.to_string(), m.to_string());
                            }
                            self.release_pending(out);
                        }
                    }
                }
            }
            "permission.asked" => return Flow::Approval,
            "session.error" => return Flow::Failed(crate::gizzi_chat_stream::gizzi_error_text(&props["error"])),
            "session.status" if props["status"]["type"] == "idle" => return Flow::Idle,
            _ => {}
        }
        Flow::Continue
    }

    fn tool(&mut self, part: &Value, out: &mut Vec<Value>) {
        let call = part["callID"].as_str().or_else(|| part["id"].as_str()).unwrap_or("").to_string();
        let name = part["tool"].as_str().or_else(|| part["name"].as_str()).unwrap_or("tool");
        let terminal = match part["state"]["status"].as_str().unwrap_or("") {
            "completed" => Some("done"),
            "error" => Some("error"),
            _ => None,
        };
        // What was said before the tool is spoken before the tool's event.
        self.flush(out);
        let phase = self.tools.entry(call.clone()).or_insert(0);
        if *phase == 0 {
            *phase = 1;
            out.push(json!({ "type": "tool", "name": name, "status": "started", "toolCallId": call }));
        }
        if let Some(status) = terminal.filter(|_| *phase == 1) {
            *phase = 2;
            out.push(json!({ "type": "tool", "name": name, "status": status, "toolCallId": call }));
        }
    }
}

fn emit(events: &UnboundedSender<Value>, out: &mut Vec<Value>) {
    for e in out.drain(..) {
        let _ = events.send(e);
    }
}

async fn abort_session(client: &Client, base: &str, session_id: &str) {
    let url = format!("{base}/v1/session/{}/abort", urlencoding::encode(session_id));
    if let Err(e) = client.post(url).json(&json!({})).send().await {
        warn!("voice turn abort failed: {e}");
    }
}

/// Send `payload` to gizzi at `base`+`path` and stream the session's events to
/// `events` while the message call runs.
pub(crate) async fn stream_gizzi_turn(client: &Client, base: &str, session_id: &str, path: &str, payload: Value, events: &UnboundedSender<Value>) -> Result<TurnReply, String> {
    match tokio::time::timeout(TURN_DEADLINE, drive(client, base, session_id, path, payload, events)).await {
        Ok(r) => r,
        Err(_) => {
            abort_session(client, base, session_id).await;
            Err("That took too long, so I stopped. Could you ask again?".to_string())
        }
    }
}

async fn drive(client: &Client, base: &str, session_id: &str, path: &str, payload: Value, events: &UnboundedSender<Value>) -> Result<TurnReply, String> {
    // Subscribe before sending so no early delta is missed.
    let mut stream = match client.get(format!("{base}/event")).header("Accept", "text/event-stream").send().await {
        Ok(r) if r.status().is_success() => Some(r.bytes_stream().boxed()),
        Ok(r) => {
            warn!(status = %r.status(), "voice turn: gizzi event stream refused; running the turn without streaming");
            None
        }
        Err(e) => {
            warn!("voice turn: gizzi event stream unreachable ({e}); running the turn without streaming");
            None
        }
    };
    let post = async {
        let res = client.post(format!("{base}{path}")).json(&payload).send().await.map_err(|e| format!("gizzi turn failed: {e}"))?;
        let status = res.status();
        let body = res.bytes().await.map_err(|e| format!("gizzi turn failed: {e}"))?;
        Ok::<_, String>((status, body))
    };
    tokio::pin!(post);

    let mut fold = TurnFold::new(session_id);
    let mut buf = String::new();
    let mut out = Vec::new();
    let mut posted: Option<(reqwest::StatusCode, bytes::Bytes)> = None;
    let mut trailing_until: Option<tokio::time::Instant> = None;
    let mut ended: Option<Flow> = None;

    while ended.is_none() {
        let chunk = async {
            match (stream.as_mut(), trailing_until) {
                (Some(s), None) => s.next().await,
                (Some(s), Some(until)) => tokio::time::timeout_at(until, s.next()).await.unwrap_or(None),
                (None, _) => futures::future::pending().await,
            }
        };
        tokio::select! {
            r = &mut post, if posted.is_none() => {
                posted = Some(r?);
                trailing_until = Some(tokio::time::Instant::now() + TRAILING_EVENTS);
                if stream.is_none() { break; }
            }
            c = chunk => match c {
                Some(Ok(bytes)) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(end) = buf.find("\n\n") {
                        let block: String = buf.drain(..end + 2).collect();
                        let Some(data) = block.lines().find_map(|l| l.strip_prefix("data:")) else { continue };
                        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { continue };
                        match fold.feed(&ev, &mut out) {
                            Flow::Continue => {}
                            Flow::Idle => { if posted.is_some() { ended = Some(Flow::Idle); } }
                            other => { ended = Some(other); }
                        }
                        emit(events, &mut out);
                        if ended.is_some() { break; }
                    }
                }
                // The bus closed (or went quiet after the message call returned).
                _ => { stream = None; if posted.is_some() { break; } }
            },
        }
        if posted.is_some() && trailing_until.is_some_and(|t| tokio::time::Instant::now() >= t) {
            break;
        }
    }
    fold.flush(&mut out);
    emit(events, &mut out);

    match ended {
        Some(Flow::Approval) => {
            abort_session(client, base, session_id).await;
            return Err(APPROVAL_SPOKEN.to_string());
        }
        Some(Flow::Failed(message)) => return Err(message),
        _ => {}
    }
    let (status, body) = match posted {
        Some(p) => p,
        None => post.await?,
    };
    if !status.is_success() {
        return Err(format!("gizzi turn failed ({status}): {}", String::from_utf8_lossy(&body)));
    }
    if fold.spoke {
        return match crate::agent_session_routes::gizzi_message_error(&body) {
            Some(error) => Err(error),
            None => Ok(TurnReply::Streamed),
        };
    }
    // Nothing streamed (the bus missed it): the final reply, as the channel path reads it.
    crate::agent_session_routes::gizzi_message_reply(&body).map(TurnReply::Final)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, response::Response, routing::{get, post}, Router};
    use std::sync::{Arc, Mutex};

    fn sse(events: &[Value]) -> String {
        events.iter().map(|e| format!("data: {e}\n\n")).collect()
    }

    /// A fake gizzi: `/event` plays `events` (then stays open), the message call
    /// answers `reply` after `delay`, and aborts are recorded.
    async fn fake_gizzi(events: Vec<Value>, reply: Value, delay: Duration) -> (String, Arc<Mutex<Vec<String>>>) {
        let aborts = Arc::new(Mutex::new(Vec::new()));
        let seen = aborts.clone();
        let body = sse(&events);
        let app = Router::new()
            .route("/event", get(move || {
                let body = body.clone();
                async move {
                    let s = futures::stream::once(async move { Ok::<_, std::io::Error>(bytes::Bytes::from(body)) }).chain(futures::stream::pending());
                    Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(s)).unwrap()
                }
            }))
            .route("/v1/session/:id/message", post(move || {
                let reply = reply.clone();
                async move { tokio::time::sleep(delay).await; axum::Json(reply) }
            }))
            .route("/v1/session/:id/abort", post(move || { let seen = seen.clone(); async move { seen.lock().unwrap().push("abort".into()); axum::Json(json!(true)) } }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), aborts)
    }

    fn part(kind: &str, id: &str) -> Value {
        json!({ "type": "message.part.updated", "properties": { "part": { "id": id, "type": kind, "sessionID": "s1", "messageID": "m2" } } })
    }
    fn delta(id: &str, text: &str) -> Value {
        json!({ "type": "message.part.delta", "properties": { "sessionID": "s1", "partID": id, "field": "text", "delta": text } })
    }
    fn tool(status: &str) -> Value {
        json!({ "type": "message.part.updated", "properties": { "part": { "id": "p9", "type": "tool", "tool": "calendar", "callID": "c1", "sessionID": "s1", "state": { "status": status } } } })
    }
    fn idle() -> Value {
        json!({ "type": "session.status", "properties": { "sessionID": "s1", "status": { "type": "idle" } } })
    }

    async fn run(events: Vec<Value>, delay: Duration) -> (Result<TurnReply, String>, Vec<Value>, Arc<Mutex<Vec<String>>>) {
        let (base, aborts) = fake_gizzi(events, json!({ "info": { "id": "m2", "sessionID": "s1", "role": "assistant" }, "parts": [{ "type": "text", "text": "final text" }] }), delay).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let r = stream_gizzi_turn(&Client::new(), &base, "s1", "/v1/session/s1/message", json!({}), &tx).await;
        drop(tx);
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            got.push(e);
        }
        (r, got, aborts)
    }

    #[tokio::test]
    async fn deltas_stream_in_order_at_word_boundaries_with_tool_steps() {
        let events = vec![
            json!({ "type": "message.updated", "properties": { "info": { "id": "m1", "role": "user", "sessionID": "s1" } } }),
            part("text", "p1"),
            delta("p1", "Let me che"),
            delta("p1", "ck. "),
            tool("running"),
            tool("completed"),
            part("text", "p2"),
            delta("p2", "You're free"),
            idle(),
        ];
        let (r, got, _) = run(events, Duration::from_millis(50)).await;
        assert_eq!(r.unwrap(), TurnReply::Streamed);
        assert_eq!(
            got,
            vec![
                json!({ "type": "text.delta", "text": "Let me " }),
                json!({ "type": "text.delta", "text": "check. " }),
                json!({ "type": "tool", "name": "calendar", "status": "started", "toolCallId": "c1" }),
                json!({ "type": "tool", "name": "calendar", "status": "done", "toolCallId": "c1" }),
                json!({ "type": "text.delta", "text": "You're " }),
                json!({ "type": "text.delta", "text": "free" }),
            ]
        );
    }

    #[tokio::test]
    async fn a_session_error_is_surfaced() {
        let events = vec![json!({ "type": "session.error", "properties": { "sessionID": "s1", "error": { "data": { "message": "model overloaded" } } } })];
        let (r, _, _) = run(events, Duration::from_millis(50)).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn an_approval_request_fails_fast_and_aborts_the_turn() {
        let events = vec![json!({ "type": "permission.asked", "properties": { "sessionID": "s1" } })];
        let (r, _, aborts) = run(events, Duration::from_secs(30)).await;
        assert_eq!(r.unwrap_err(), APPROVAL_SPOKEN);
        assert_eq!(aborts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn other_sessions_events_are_ignored_and_a_silent_bus_falls_back_to_the_final_reply() {
        let other = json!({ "type": "message.part.delta", "properties": { "sessionID": "other", "partID": "p1", "field": "text", "delta": "nope " } });
        let (r, got, _) = run(vec![other], Duration::from_millis(10)).await;
        assert_eq!(r.unwrap(), TurnReply::Final("final text".into()));
        assert!(got.is_empty());
    }
}
