//! Discord Gateway (websocket) client. Started only for Discord provider
//! accounts whose sealed secret carries a `botToken`; without one nothing runs
//! (the interactions/webhook path in `channel_transports` is unaffected).
//!
//! Protocol: HELLO -> heartbeat loop (with ACK tracking, a missed ACK is a
//! zombie connection and reconnects) -> IDENTIFY with intents, or RESUME with
//! `session_id` + last `seq` when we hold a session. Dispatch events
//! (MESSAGE_CREATE/UPDATE/DELETE, reactions) are handed on unchanged as
//! `{t, s, d}`, which is exactly the shape `discord_normalize` consumes.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::{interval_at, Instant};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::channel_transports::{accounts, build_transport, dispatch_events, pick, ReqwestSend};
use crate::AppState;

pub const GATEWAY_URL: &str = "wss://gateway.discord.gg";
pub const GUILDS: u64 = 1 << 0;
pub const GUILD_MESSAGES: u64 = 1 << 9;
pub const GUILD_MESSAGE_REACTIONS: u64 = 1 << 10;
pub const DIRECT_MESSAGES: u64 = 1 << 12;
pub const DIRECT_MESSAGE_REACTIONS: u64 = 1 << 13;
/// Privileged: must also be enabled for the bot in the Discord developer portal.
pub const MESSAGE_CONTENT: u64 = 1 << 15;
pub const DEFAULT_INTENTS: u64 = GUILDS | GUILD_MESSAGES | GUILD_MESSAGE_REACTIONS | DIRECT_MESSAGES | DIRECT_MESSAGE_REACTIONS | MESSAGE_CONTENT;

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub session_id: Option<String>,
    pub seq: Option<i64>,
    pub resume_url: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Reconnect (resuming when the session survived).
    Reconnect,
    /// The server closed us or the socket dropped.
    Closed,
}

fn with_query(url: &str) -> String {
    if url.contains('?') {
        url.to_string()
    } else {
        format!("{}/?v=10&encoding=json", url.trim_end_matches('/'))
    }
}

/// One websocket connection. Events are delivered to `on_event` as `{t, s, d}`.
pub async fn run_session<F>(url: &str, token: &str, intents: u64, session: &mut Session, on_event: F) -> Result<Outcome, String>
where
    F: Fn(Value) + Send,
{
    let target = session.resume_url.clone().filter(|_| session.session_id.is_some()).unwrap_or_else(|| url.to_string());
    let (mut ws, _) = tokio_tungstenite::connect_async(with_query(&target)).await.map_err(|e| e.to_string())?;

    // HELLO
    let hello = loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                if v["op"] == 10 {
                    break v;
                }
            }
            Some(Ok(_)) => continue,
            _ => return Ok(Outcome::Closed),
        }
    };
    let every = Duration::from_millis(hello["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250).max(10));

    let first = if let (Some(sid), Some(seq)) = (session.session_id.clone(), session.seq) {
        json!({ "op": 6, "d": { "token": token, "session_id": sid, "seq": seq } })
    } else {
        json!({ "op": 2, "d": { "token": token, "intents": intents, "properties": { "os": std::env::consts::OS, "browser": "allternit", "device": "allternit" } } })
    };
    ws.send(Message::Text(first.to_string())).await.map_err(|e| e.to_string())?;

    // The first beat is jittered per the docs; a fixed fraction keeps this deterministic.
    let mut beat = interval_at(Instant::now() + every / 2, every);
    let mut acked = true;
    loop {
        tokio::select! {
            _ = beat.tick() => {
                if !acked {
                    warn!("discord gateway: heartbeat not acknowledged; reconnecting");
                    let _ = ws.close(None).await;
                    return Ok(Outcome::Reconnect);
                }
                acked = false;
                ws.send(Message::Text(json!({ "op": 1, "d": session.seq }).to_string())).await.map_err(|e| e.to_string())?;
            }
            msg = ws.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; continue; }
                    Some(Ok(Message::Close(_))) | None => return Ok(Outcome::Closed),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.to_string()),
                };
                let v: Value = match serde_json::from_str(&text) { Ok(v) => v, Err(_) => continue };
                if let Some(s) = v["s"].as_i64() {
                    session.seq = Some(s);
                }
                match v["op"].as_i64().unwrap_or(-1) {
                    0 => match v["t"].as_str().unwrap_or_default() {
                        "READY" => {
                            session.session_id = v["d"]["session_id"].as_str().map(str::to_string);
                            session.resume_url = v["d"]["resume_gateway_url"].as_str().map(str::to_string);
                        }
                        "RESUMED" => {}
                        _ => on_event(v),
                    },
                    1 => ws.send(Message::Text(json!({ "op": 1, "d": session.seq }).to_string())).await.map_err(|e| e.to_string())?,
                    11 => acked = true,
                    7 => {
                        let _ = ws.close(None).await;
                        return Ok(Outcome::Reconnect);
                    }
                    9 => {
                        // d = true: the session may still be resumable; false: start over.
                        if v["d"].as_bool() != Some(true) {
                            *session = Session::default();
                        }
                        let _ = ws.close(None).await;
                        return Ok(Outcome::Reconnect);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Reconnect loop with backoff; runs until the task is dropped.
pub async fn run_forever<F>(url: String, token: String, intents: u64, on_event: F)
where
    F: Fn(Value) + Send + Sync + 'static,
{
    let mut session = Session::default();
    let mut failures: u32 = 0;
    loop {
        match run_session(&url, &token, intents, &mut session, &on_event).await {
            Ok(Outcome::Reconnect) => failures = 0,
            Ok(Outcome::Closed) => failures += 1,
            Err(e) => {
                warn!("discord gateway: {e}");
                failures += 1;
            }
        }
        let wait = if failures == 0 { 1 } else { (1u64 << failures.min(6)).min(60) };
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

/// Start a gateway client for every Discord account that has a `botToken`.
/// No such account -> nothing is started. `ALLTERNIT_DISCORD_GATEWAY=0` disables it.
pub fn spawn_bound(state: Arc<AppState>) {
    if std::env::var("ALLTERNIT_DISCORD_GATEWAY").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    tokio::spawn(async move {
        let bound: Vec<_> = accounts(&state.db, "discord", None).into_iter().filter(|a| !pick(&a.secret, "botToken").is_empty()).collect();
        for acct in bound {
            let token = pick(&acct.secret, "botToken");
            let Some(tx) = build_transport("discord", &acct.secret, Arc::new(ReqwestSend)) else { continue };
            info!(account = %acct.id, "starting Discord gateway client");
            let st = state.clone();
            let handle = tokio::runtime::Handle::current();
            tokio::spawn(run_forever(GATEWAY_URL.to_string(), token, DEFAULT_INTENTS, move |payload| {
                let (st, acct, tx) = (st.clone(), acct.clone(), tx.clone());
                let events = tx.normalize(&payload);
                if events.is_empty() {
                    return;
                }
                handle.spawn(async move { dispatch_events(&st, &acct, tx, events).await });
            }));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    async fn send(ws: &mut Ws, v: Value) {
        ws.send(Message::Text(v.to_string())).await.unwrap();
    }
    async fn recv(ws: &mut Ws) -> Value {
        loop {
            if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }
    async fn accept(l: &TcpListener, interval_ms: u64) -> Ws {
        let (s, _) = l.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        send(&mut ws, json!({ "op": 10, "d": { "heartbeat_interval": interval_ms } })).await;
        ws
    }

    #[tokio::test]
    async fn identify_heartbeat_dispatch_reconnect_and_resume() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", l.local_addr().unwrap());
        let server_url = url.clone();
        let server = tokio::spawn(async move {
            // Connection 1: IDENTIFY with intents, then READY, a message, heartbeat carrying seq, then op 7.
            // 500 ms, not 40: the client reconnects when a heartbeat goes unacknowledged, and on a
            // slow CI runner the fake server could not ack within 40 ms (reset mid-test, 2026-09-30).
            let mut ws = accept(&l, 500).await;
            let id = recv(&mut ws).await;
            assert_eq!(id["op"], 2);
            assert_eq!(id["d"]["token"], "bot-tok");
            assert_eq!(id["d"]["intents"], DEFAULT_INTENTS);
            send(&mut ws, json!({ "op": 0, "s": 1, "t": "READY", "d": { "session_id": "sess-1", "resume_gateway_url": server_url } })).await;
            send(&mut ws, json!({ "op": 0, "s": 2, "t": "MESSAGE_CREATE", "d": { "id": "m1", "channel_id": "c1", "content": "hi", "author": { "id": "u1" } } })).await;
            // The heartbeat interval is 40 ms, so on a slow runner a heartbeat can go out before the
            // client has processed seq 2 (CI flake 2026-09-30). Skip those; the one that follows must carry 2.
            let hb = loop {
                let hb = recv(&mut ws).await;
                assert_eq!(hb["op"], 1);
                if hb["d"] == 2 {
                    break hb;
                }
                assert!(hb["d"].is_null() || hb["d"] == 1, "heartbeat seq never goes backwards: {hb}");
                send(&mut ws, json!({ "op": 11 })).await;
            };
            assert_eq!(hb["d"], 2);
            send(&mut ws, json!({ "op": 11 })).await;
            send(&mut ws, json!({ "op": 7 })).await;
            // Connection 2: RESUME with session id and last seq; then edit, delete, reaction.
            let mut ws = accept(&l, 5000).await;
            let r = recv(&mut ws).await;
            assert_eq!(r["op"], 6);
            assert_eq!(r["d"]["session_id"], "sess-1");
            assert_eq!(r["d"]["seq"], 2);
            assert_eq!(r["d"]["token"], "bot-tok");
            send(&mut ws, json!({ "op": 0, "s": 3, "t": "RESUMED", "d": {} })).await;
            send(&mut ws, json!({ "op": 0, "s": 4, "t": "MESSAGE_UPDATE", "d": { "id": "m1", "channel_id": "c1", "content": "hi!", "edited_timestamp": "x" } })).await;
            send(&mut ws, json!({ "op": 0, "s": 5, "t": "MESSAGE_DELETE", "d": { "id": "m1", "channel_id": "c1" } })).await;
            send(&mut ws, json!({ "op": 0, "s": 6, "t": "MESSAGE_REACTION_ADD", "d": { "user_id": "u2", "channel_id": "c1", "message_id": "m1", "emoji": { "name": "fire" } } })).await;
            // Invalid session (not resumable): the client must fall back to IDENTIFY.
            send(&mut ws, json!({ "op": 9, "d": false })).await;
            let mut ws = accept(&l, 5000).await;
            assert_eq!(recv(&mut ws).await["op"], 2);
            ws.close(None).await.ok();
        });

        let got: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(vec![]));
        let sink = got.clone();
        let on = move |v: Value| sink.lock().unwrap().push(v);
        let mut session = Session::default();
        assert_eq!(run_session(&url, "bot-tok", DEFAULT_INTENTS, &mut session, &on).await.unwrap(), Outcome::Reconnect);
        assert_eq!((session.session_id.as_deref(), session.seq), (Some("sess-1"), Some(2)));
        assert_eq!(run_session(&url, "bot-tok", DEFAULT_INTENTS, &mut session, &on).await.unwrap(), Outcome::Reconnect);
        assert!(session.session_id.is_none(), "op 9 with d=false drops the session");
        assert_eq!(run_session(&url, "bot-tok", DEFAULT_INTENTS, &mut session, &on).await.unwrap(), Outcome::Closed);
        server.await.unwrap();

        // The dispatches normalize through the existing Discord normalization.
        use crate::channel_gateway::InboundKind::*;
        let kinds: Vec<_> = got.lock().unwrap().iter().flat_map(crate::channel_transports::discord_normalize).map(|e| (e.kind, e.conversation)).collect();
        assert_eq!(
            kinds,
            vec![(Message, "discord:c1".to_string()), (Edited, "discord:c1".to_string()), (Deleted, "discord:c1".to_string()), (ReactionUpdated, "discord:c1".to_string())]
        );
    }

    #[tokio::test]
    async fn a_missed_heartbeat_ack_reconnects() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", l.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&l, 30).await;
            let _identify = recv(&mut ws).await;
            // Never ACK: first beat arrives, second tick finds it unacknowledged.
            assert_eq!(recv(&mut ws).await["op"], 1);
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let mut session = Session::default();
        let out = run_session(&url, "t", DEFAULT_INTENTS, &mut session, |_| {}).await.unwrap();
        assert_eq!(out, Outcome::Reconnect);
        server.await.unwrap();
    }

    #[test]
    fn query_is_added_once() {
        assert_eq!(with_query("wss://gateway.discord.gg"), "wss://gateway.discord.gg/?v=10&encoding=json");
        assert_eq!(with_query("ws://127.0.0.1:1/?x=1"), "ws://127.0.0.1:1/?x=1");
    }
}
