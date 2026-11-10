//! Runtime status snapshot for the iPhone system surfaces (Live Activity,
//! widget, badge, revoke by diff). The cloud keeps the latest one per runtime;
//! it is never pushed as an alert (the cloud decides what to do with it).
//!
//! - **Shape** (`status.snapshot`, docs `api/runtime-events.mdx`):
//!   `activeBots`, `attentionBots`, `pendingApprovals`, `paused`, `focus`,
//!   `bots` (≤ 20), `needsYou` (every open inbox id, for the revoke diff),
//!   `needsYouRows` (≤ 4, `line` ≤ 80 characters), `loops` (left out when the
//!   last poll failed), `lastRoutine`. `paused` is the owner's "Pause all bots"
//!   switch (`bots_pause`); flipping it wakes the producer.
//! - **Focus bot:** the oldest pending approval, then a bot that needs input,
//!   then a blocked or failed bot, then the most recently active working bot.
//!   `focus.private` is true when the item is addressed to another person, so
//!   its text must not travel in plain pushes.
//! - **Delivery:** posted on its own to `POST /api/v1/runtime/events` (same
//!   relay signature as the forwarder), only when it changed, at most one per
//!   5 s. A cloud that does not know the type answers 4xx; the producer then
//!   waits 10 minutes, and the ledger forwarder is not affected.
//! - **Loops:** `GET {gizzi}/v1/automations/loops` on loopback, every 5 s while
//!   a loop runs and every 60 s otherwise.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::db::DbHandle;
use crate::inbox_needs::{self, APPROVAL_KINDS};
use crate::relay_auth::RelaySecret;
use crate::runtime_events::{request_headers, EventSink, HttpSink, EVENTS_PATH};
use crate::AppState;

pub const TYPE: &str = "status.snapshot";
pub const MAX_BOTS: usize = 20;
pub const MAX_ROWS: usize = 4;
pub const MAX_LINE: usize = 80;
const MIN_GAP: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_secs(5);
const LOOP_POLL_RUNNING: Duration = Duration::from_secs(5);
const LOOP_POLL_IDLE: Duration = Duration::from_secs(60);
const REFUSED_WAIT: Duration = Duration::from_secs(600);
const MAX_LOOPS: usize = 5;

fn cut(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n - 1).collect::<String>())
    }
}

fn parse_ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok().map(|n| n.and_utc()))
}

/// Rank of a bot status for the `bots` list (lower first).
fn rank(status: &str) -> u8 {
    match status {
        "waiting_approval" => 0,
        "needs_input" => 1,
        "blocked" | "failed" => 2,
        "working" => 3,
        _ => 4,
    }
}

struct Thread {
    id: String,
    bot: String,
    status: String,
    line: String,
    at: String,
}

/// Build the snapshot from the runtime's own stores. `loops` is the last
/// successful poll (`None` leaves the key out).
pub fn build(conn: &Connection, owner: &str, loops: Option<&[Value]>) -> rusqlite::Result<Value> {
    let items = inbox_needs::collect(conn, owner, inbox_needs::DEFAULT_UNANSWERED_MINUTES)?;
    let approvals: Vec<&Value> = items.iter().filter(|i| APPROVAL_KINDS.contains(&i["kind"].as_str().unwrap_or(""))).collect();

    let mut st = conn.prepare("SELECT id, COALESCE(json_extract(config, '$.botProfile.displayName'), name, '') FROM agents WHERE user_id = ?1 AND is_bot = 1 ORDER BY name")?;
    let bots: Vec<(String, String)> = st.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;

    // Threads that say something about a bot right now. Blocked and failed
    // count for a day; working, needs-you and review until they move on.
    let day_ago = chrono::Utc::now() - chrono::Duration::hours(24);
    let mut st = conn.prepare(
        "SELECT id, bot_id, status, COALESCE(status_line, ''), COALESCE(last_activity_at, updated_at, '') FROM bot_threads
         WHERE user_id = ?1 AND incognito = 0 AND status IN ('needs_you', 'review', 'working', 'queued', 'blocked', 'failed')
         ORDER BY updated_at DESC LIMIT 500",
    )?;
    let threads: Vec<Thread> = st
        .query_map(params![owner], |r| Ok(Thread { id: r.get(0)?, bot: r.get(1)?, status: r.get(2)?, line: r.get(3)?, at: r.get(4)? }))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|t| !matches!(t.status.as_str(), "blocked" | "failed") || parse_ts(&t.at).is_some_and(|a| a >= day_ago))
        .collect();
    let norm = |s: &str| match s {
        "needs_you" | "review" => "needs_input",
        "queued" => "working",
        other => other,
    }
    .to_string();

    // One status per bot: its most pressing thread (or a pending approval).
    let mut per_bot: Vec<Value> = bots
        .iter()
        .map(|(id, name)| {
            let approval = approvals.iter().any(|a| a["botId"] == id.as_str());
            let best = threads.iter().filter(|t| &t.bot == id).min_by_key(|t| rank(&norm(&t.status)));
            let status = if approval { "waiting_approval".to_string() } else { best.map(|t| norm(&t.status)).unwrap_or_else(|| "idle".into()) };
            json!({ "botId": id, "name": name, "status": status, "step": best.map(|t| cut(&t.line, MAX_LINE)).filter(|s| !s.is_empty()) })
        })
        .collect();
    per_bot.sort_by_key(|b| rank(b["status"].as_str().unwrap_or("")));
    let count = |f: &dyn Fn(&str) -> bool| per_bot.iter().filter(|b| f(b["status"].as_str().unwrap_or(""))).count();
    let active = count(&|s| s == "working");
    let attention = count(&|s| rank(s) <= 2);
    let name_of = |bot: &str| bots.iter().find(|(id, _)| id == bot).map(|(_, n)| n.clone()).unwrap_or_default();
    let session_of = |thread: &str| -> Option<String> {
        conn.query_row("SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1", params![thread], |r| r.get(0)).optional().ok().flatten()
    };

    // Focus: oldest approval, then needs input, then blocked/failed, then the latest working bot.
    let focus = if let Some(a) = approvals.iter().min_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str())) {
        let bot = a["botId"].as_str().unwrap_or("");
        let keys = inbox_needs::notify_keys(a);
        let step = threads.iter().find(|t| t.bot == bot && !t.line.is_empty()).map(|t| cut(&t.line, MAX_LINE));
        Some(json!({
            "botId": bot, "botName": a["botName"], "status": "waiting_approval", "step": step,
            "inboxId": a["id"], "sessionId": keys.get("sessionId"), "question": cut(a["summary"].as_str().unwrap_or(""), 140),
            "contentHash": a["contentHash"], "approveInline": a["approveInline"], "private": inbox_needs::is_private(a),
        }))
    } else {
        let pick = |want: &[&str], oldest: bool| {
            let mut c: Vec<&Thread> = threads.iter().filter(|t| want.contains(&t.status.as_str())).collect();
            c.sort_by(|a, b| a.at.cmp(&b.at));
            if oldest { c.first().copied() } else { c.last().copied() }
        };
        pick(&["needs_you", "review"], true)
            .or_else(|| pick(&["blocked", "failed"], false))
            .or_else(|| pick(&["working", "queued"], false))
            .map(|t| {
                let inbox = items.iter().find(|i| i["threadId"] == t.id.as_str());
                json!({
                    "botId": t.bot, "botName": name_of(&t.bot), "status": norm(&t.status), "step": Some(cut(&t.line, MAX_LINE)).filter(|s| !s.is_empty()),
                    "inboxId": inbox.map(|i| i["id"].clone()), "sessionId": session_of(&t.id),
                    "question": inbox.map(|i| cut(i["summary"].as_str().unwrap_or(""), 140)),
                    "private": inbox.is_some_and(inbox_needs::is_private),
                })
            })
    };

    let rows: Vec<Value> = items
        .iter()
        .take(MAX_ROWS)
        .map(|i| json!({ "id": i["id"], "kind": i["kind"], "botName": i["botName"], "line": cut(i["summary"].as_str().unwrap_or(""), MAX_LINE), "private": inbox_needs::is_private(i) }))
        .collect();

    let last_routine: Option<Value> = conn
        .query_row(
            "SELECT e.event_type, e.payload, e.occurred_at FROM bot_events e JOIN agents a ON a.id = e.bot_id
             WHERE a.user_id = ?1 AND e.event_type IN ('routine.completed', 'routine.failed') ORDER BY e.rowid DESC LIMIT 1",
            params![owner],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .optional()?
        .map(|(ty, p, at)| {
            let p: Value = serde_json::from_str(&p).unwrap_or(Value::Null);
            json!({
                "routineId": p["routineId"], "title": p["title"],
                "status": if ty == "routine.completed" { "completed" } else { "failed" },
                "at": parse_ts(&at).map(|d| d.timestamp()),
            })
        });

    per_bot.truncate(MAX_BOTS);
    let mut snap = json!({
        "activeBots": active,
        "attentionBots": attention,
        "pendingApprovals": approvals.len(),
        "paused": crate::bots_pause::is_paused(conn, owner),
        "focus": focus,
        "bots": per_bot,
        "needsYou": items.iter().map(|i| i["id"].clone()).collect::<Vec<_>>(),
        "needsYouRows": rows,
        "lastRoutine": last_routine,
    });
    if let Some(l) = loops {
        snap["loops"] = json!(l);
    }
    Ok(snap)
}

/// gizzi-code's loop rows → the snapshot's `loops` (running and paused only).
pub fn loops_from_gizzi(rows: &Value) -> Vec<Value> {
    rows.as_array()
        .map(|a| {
            a.iter()
                .filter(|l| matches!(l["state"].as_str(), Some("running" | "paused")))
                .take(MAX_LOOPS)
                .map(|l| {
                    json!({
                        "loopId": l["id"],
                        "command": cut(l["command"].as_str().unwrap_or(""), MAX_LINE),
                        "iterationsCompleted": l["iteration_log"].as_array().map(|x| x.len()).unwrap_or(0),
                        "maxIterations": l["max_iterations"],
                        "state": l["state"],
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Sends a snapshot when it changed. One per runtime process.
#[derive(Default)]
pub struct Producer {
    last_hash: Option<String>,
    last_sent: Option<Instant>,
    hold_until: Option<Instant>,
    /// Last successful loop poll; `None` after a failed one.
    pub loops: Option<Vec<Value>>,
    next_poll: Option<Instant>,
}

fn hash(v: &Value) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(v.to_string().as_bytes()))
}

impl Producer {
    /// Build and send if it changed. `Ok(true)` when the cloud took one.
    pub async fn tick(&mut self, db: &DbHandle, secret: &dyn RelaySecret, sink: &dyn EventSink, cloud: &str, now: Instant) -> Result<bool, String> {
        let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
            return Ok(false);
        };
        if self.hold_until.is_some_and(|h| now < h) || self.last_sent.is_some_and(|t| now.duration_since(t) < MIN_GAP) {
            return Ok(false);
        }
        let (db2, o, loops) = (db.clone(), owner.clone(), self.loops.clone());
        let snap = tokio::task::spawn_blocking(move || db2.connect().and_then(|c| build(&c, &o, loops.as_deref())))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let h = hash(&snap);
        if self.last_hash.as_deref() == Some(h.as_str()) {
            return Ok(false);
        }
        let at = chrono::Utc::now();
        // A new id per send: the cloud drops repeated ids, and A → B → A must arrive.
        let ev = json!({ "id": format!("rt:snap:{}", at.timestamp_millis()), "type": TYPE, "at": at.to_rfc3339(), "data": snap });
        let body = serde_json::to_vec(&json!({ "events": [ev] })).map_err(|e| e.to_string())?;
        let headers = request_headers(&token, &owner, &runtime_id, &body);
        match sink.post(&format!("{cloud}{EVENTS_PATH}"), headers, body).await {
            Ok(s) if (200..300).contains(&s) => {
                self.last_hash = Some(h);
                self.last_sent = Some(now);
                Ok(true)
            }
            Ok(s) if (400..500).contains(&s) && s != 429 => {
                // An older cloud that doesn't take the type: try again later.
                self.hold_until = Some(now + REFUSED_WAIT);
                Err(format!("the cloud refused the snapshot ({s})"))
            }
            Ok(s) => Err(format!("the cloud answered {s}")),
            Err(e) => Err(e),
        }
    }

    /// Poll gizzi-code's loops when due (5 s while one runs, 60 s otherwise).
    pub async fn poll_loops(&mut self, now: Instant) {
        if self.next_poll.is_some_and(|n| now < n) {
            return;
        }
        let url = format!("{}/v1/automations/loops", crate::agency_api::executor::gizzi_url());
        let client = crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new());
        let got = match client.get(&url).timeout(Duration::from_secs(3)).send().await {
            Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
            _ => None,
        };
        let was_running = self.loops.as_ref().is_some_and(|l| l.iter().any(|x| x["state"] == "running"));
        self.loops = got.as_ref().map(loops_from_gizzi);
        let running = self.loops.as_ref().is_some_and(|l| l.iter().any(|x| x["state"] == "running"));
        self.next_poll = Some(now + if running || (self.loops.is_none() && was_running) { LOOP_POLL_RUNNING } else { LOOP_POLL_IDLE });
    }
}

/// Background producer (inert until the runtime is paired).
pub async fn run(state: Arc<AppState>) {
    let secret = crate::relay_auth::process_secret();
    let mut p = Producer::default();
    loop {
        // The pause switch wakes the producer at once (still at most one send per MIN_GAP).
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = crate::bots_pause::CHANGED.notified() => {}
        }
        if secret.paired_owner().is_none() {
            continue;
        }
        let now = Instant::now();
        p.poll_loops(now).await;
        if let Err(e) = p.tick(&state.db, secret.as_ref(), &HttpSink, &crate::phone_sync::cloud_base(), now).await {
            tracing::debug!("status snapshot: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct Paired;
    impl RelaySecret for Paired {
        fn device_token(&self) -> Option<String> {
            Some("tok".into())
        }
        fn paired_owner(&self) -> Option<String> {
            Some("user-a".into())
        }
        fn runtime_id(&self) -> Option<String> {
            Some("rt_test".into())
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Value>>);
    #[async_trait]
    impl EventSink for Recorder {
        async fn post(&self, _url: &str, _h: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String> {
            let v: Value = serde_json::from_slice(&body).unwrap();
            self.0.lock().unwrap().push(v["events"][0].clone());
            Ok(202)
        }
    }

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-snap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        for (id, n) in [("b-work", "Worker"), ("b-block", "Blocked"), ("b-input", "Asker"), ("b-appr", "Raven")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'user-a', ?2, 'm', 'p', 1, '{}')", params![id, n]).unwrap();
        }
        state
    }

    fn thread(c: &Connection, id: &str, bot: &str, status: &str, line: &str, min_ago: i64) {
        let t = (chrono::Utc::now() - chrono::Duration::minutes(min_ago)).to_rfc3339();
        c.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, title, status, status_line, last_activity_at, created_at, updated_at) VALUES (?1,'user-a',?2,?1,?3,?4,?5,?5,?5)",
            params![id, bot, status, line, t],
        )
        .unwrap();
    }

    fn set_status(c: &Connection, id: &str, status: &str) {
        c.execute("UPDATE bot_threads SET status = ?2 WHERE id = ?1", params![id, status]).unwrap();
    }

    #[tokio::test]
    async fn focus_follows_the_agreed_order() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        thread(&c, "t-work", "b-work", "working", "Drafting the reply", 1);
        thread(&c, "t-block", "b-block", "blocked", "Waiting on a login", 5);
        thread(&c, "t-input", "b-input", "needs_you", "Which date?", 9);
        thread(&c, "t-appr", "b-appr", "needs_you", "Approval needed", 2);
        let cx = crate::gateway_runner::Cx { owner: "user-a".into(), thread_id: "t-appr".into(), bot_id: "b-appr".into(), generation: 0, session_id: "s1".into(), exec: json!({}) };
        let ap = crate::gateway_runner::create_approval(&st.db, &cx, "allternit", "post to slack", None, json!({ "text": "Hi Sam, the invoice is attached" }), None).unwrap_or_else(|_| panic!("create_approval"));
        let focus = |c: &Connection| build(c, "user-a", None).unwrap()["focus"].clone();

        let f = focus(&c);
        assert_eq!((f["botId"].as_str(), f["status"].as_str(), f["inboxId"].as_str()), (Some("b-appr"), Some("waiting_approval"), Some(format!("approval:{ap}").as_str())));
        assert_eq!(f["private"], true, "a post to a conversation is addressed to someone");
        assert_eq!(f["approveInline"], true);
        assert_eq!(f["contentHash"].as_str().map(str::len), Some(32));

        c.execute("UPDATE gateway_approvals SET state = 'approved' WHERE id = ?1", params![ap]).unwrap();
        set_status(&c, "t-appr", "idle");
        assert_eq!(focus(&c)["botId"], "b-input");
        set_status(&c, "t-input", "idle");
        assert_eq!((focus(&c)["botId"].as_str(), focus(&c)["status"].as_str()), (Some("b-block"), Some("blocked")));
        set_status(&c, "t-block", "idle");
        let f = focus(&c);
        assert_eq!((f["botId"].as_str(), f["step"].as_str(), f["private"].as_bool()), (Some("b-work"), Some("Drafting the reply"), Some(false)));

        let snap = build(&c, "user-a", None).unwrap();
        assert_eq!(snap["activeBots"], 1);
        assert_eq!(snap["bots"][0]["botId"], "b-work", "working ranks above idle");
        assert!(snap.get("loops").is_none(), "no successful poll: loops left out");
    }

    #[tokio::test]
    async fn sent_only_when_it_changes_and_not_more_than_every_5s() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        let sink = Recorder::default();
        let mut p = Producer::default();
        let t0 = Instant::now();
        assert!(p.tick(&st.db, &Paired, &sink, "https://cloud", t0).await.unwrap());
        // Nothing changed: nothing sent, even well after the gap.
        assert!(!p.tick(&st.db, &Paired, &sink, "https://cloud", t0 + Duration::from_secs(30)).await.unwrap());

        thread(&c, "t-work", "b-work", "working", "Reading mail", 1);
        // Changed, but inside the 5 s gap after the last send... (last send was t0)
        assert!(!p.tick(&st.db, &Paired, &sink, "https://cloud", t0 + Duration::from_secs(2)).await.unwrap());
        assert!(p.tick(&st.db, &Paired, &sink, "https://cloud", t0 + Duration::from_secs(31)).await.unwrap());

        let sent = sink.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["type"], TYPE);
        assert_eq!(sent[0]["data"]["activeBots"], 0);
        assert_eq!(sent[1]["data"]["activeBots"], 1);
        assert_ne!(sent[0]["id"], sent[1]["id"]);
    }

    #[test]
    fn loops_keep_running_and_paused_only() {
        let rows = json!([
            { "id": "l1", "command": "cargo test", "max_iterations": 10, "iteration_log": [{}, {}, {}], "state": "running" },
            { "id": "l2", "command": "old", "max_iterations": 5, "iteration_log": [], "state": "completed" },
        ]);
        let l = loops_from_gizzi(&rows);
        assert_eq!(l, vec![json!({ "loopId": "l1", "command": "cargo test", "iterationsCompleted": 3, "maxIterations": 10, "state": "running" })]);
    }
}
