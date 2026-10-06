//! Runtime → cloud event forwarder (Allternit Events P3).
//!
//! Every user-facing event on this runtime lands in a ledger first:
//! `bot_events` (everything a bot does) or `runtime_user_events` (owner-level
//! events with no bot: Subscriptions logins, chat permission approvals). This
//! module forwards the rows that matter up to the cloud event backbone, so push,
//! Platform API webhooks and MCP Events subscribers see them.
//!
//! - **One classification.** A row is forwarded when
//!   [`crate::events_feed::classify`] gives it a [`Kind::registry`] name: the
//!   same function the Desktop feed uses, so the two can't disagree.
//! - **Scope.** Only rows of the cloud account this runtime is paired as
//!   (`agents.user_id` / `runtime_user_events.user_id` = paired owner). Rows on
//!   incognito threads never leave the runtime. Unpaired runtime: nothing runs.
//! - **Delivery.** `POST {ALLTERNIT_CLOUD_API_URL}/api/v1/runtime/events`,
//!   body `{"events":[{id,type,at,bot_id?,thread_id?,data}]}` (≤ 100 events),
//!   signed with the relay scheme (`x-allternit-runtime-sig`,
//!   [`crate::relay_auth::signed_headers`]) plus `x-allternit-runtime-id`,
//!   which the cloud uses to find the device key. The device token itself is
//!   never sent. The cloud's ingest is `routes/runtime_events.rs` in cloud-api.
//! - **Exactly-once-ish.** Ids are deterministic (`rt:be:<bot_events.id>`,
//!   `rt:ue:<runtime_user_events.id>`; approvals use
//!   `rt:approval:<approvalId>:<requested|resolved>` so the same approval from
//!   two stores is one event). The cloud is idempotent on id. The cursor
//!   (`cloud_event_cursors`, V236) advances only after a 2xx, so a crash or a
//!   failed post replays the batch and the cloud drops the duplicates.
//! - **Start.** A new cursor starts at the ledger's current end: pairing a
//!   runtime does not replay its history.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::db::DbHandle;
use crate::events_feed::{classify, Kind};
use crate::relay_auth::RelaySecret;
use crate::AppState;

pub const EVENTS_PATH: &str = "/api/v1/runtime/events";
pub const SOURCE_BOT: &str = "bot_events";
pub const SOURCE_USER: &str = "runtime_user_events";
const BATCH: i64 = 100;
const POLL: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const TEXT_PREVIEW: usize = 240;

/// Payload keys copied into the forwarded `data` (an allowlist: nothing else
/// in a ledger payload leaves the runtime). `text` is cut to a preview.
const DATA_KEYS: &[&str] = &[
    "approvalId", "authority", "action", "state", "decision", "source", "remoteRef",
    "ticketId", "vendorBotId", "deadlineAt", "lane", "summary",
    "runId", "run_id", "routineId", "title", "trigger", "durationMs",
    "durationSec", "reason", "missed", "answered", "direction", "from", "channel", "provider",
    "itemId", "kind", "severity", "actionUrl",
    "loginId", "health", "previous", "label",
    "taskId", "capability", "detail", "threadId",
];

// ---------------------------------------------------------------- owner ledger

/// Append an owner-level event (no bot). Idempotent per `(user_id, key)`.
/// Returns `true` when a new row was written.
pub fn record_user_event(conn: &Connection, user_id: &str, event_type: &str, thread_id: Option<&str>, payload: &Value, key: Option<&str>) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO runtime_user_events (id, user_id, event_type, thread_id, payload, idempotency_key, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![uuid::Uuid::new_v4().to_string(), user_id, event_type, thread_id, payload.to_string(), key, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------- mapping

/// One ledger row as the forwarder reads it.
#[derive(Debug, Clone)]
pub struct LedgerRow {
    pub rowid: i64,
    pub id: String,
    pub event_type: String,
    pub bot_id: Option<String>,
    pub bot_name: Option<String>,
    pub thread_id: Option<String>,
    pub thread_title: Option<String>,
    pub payload: Value,
    pub occurred_at: String,
}

/// The registry envelope for a row, or `None` when the row stays local.
pub fn envelope(source: &str, row: &LedgerRow) -> Option<Value> {
    let kind = classify(&row.event_type, &row.payload)?;
    let ty = kind.registry()?;
    let p = &row.payload;
    let mut data = Map::new();
    for k in DATA_KEYS {
        if let Some(v) = p.get(*k).filter(|v| !v.is_null()) {
            data.insert((*k).to_string(), v.clone());
        }
    }
    if let Some(t) = p["text"].as_str() {
        data.insert("text".into(), json!(t.chars().take(TEXT_PREVIEW).collect::<String>()));
    }
    data.insert("ledgerType".into(), json!(row.event_type));
    if let Kind::CallEnded { missed } = kind {
        data.insert("missed".into(), json!(missed));
    }
    if kind == Kind::SubscriptionTaskNeedsUser {
        data.insert("source".into(), json!("subscriptions"));
    }
    if let Some(n) = &row.bot_name {
        data.insert("botName".into(), json!(n));
    }
    if let Some(t) = &row.thread_title {
        data.insert("threadTitle".into(), json!(t));
    }
    // The cloud takes ids of at most 128 characters.
    let approval = p["approvalId"].as_str().filter(|s| !s.is_empty() && s.len() <= 100);
    let id = match (kind, approval) {
        (Kind::ApprovalRequested, Some(a)) => format!("rt:approval:{a}:requested"),
        (Kind::ApprovalResolved, Some(a)) => format!("rt:approval:{a}:resolved"),
        _ if source == SOURCE_USER => format!("rt:ue:{}", row.id),
        _ => format!("rt:be:{}", row.id),
    };
    let at = chrono::DateTime::parse_from_rfc3339(&row.occurred_at)
        .map(|d| d.with_timezone(&chrono::Utc).to_rfc3339())
        .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());
    let mut ev = json!({ "id": id, "type": ty, "at": at, "data": Value::Object(data) });
    if let Some(b) = &row.bot_id {
        ev["bot_id"] = json!(b);
    }
    if let Some(t) = row.thread_id.as_deref().filter(|t| !t.is_empty()) {
        ev["thread_id"] = json!(t);
    }
    Some(ev)
}

// ---------------------------------------------------------------- cursor + reads

pub fn cursor(conn: &Connection, account: &str, source: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT last_rowid FROM cloud_event_cursors WHERE account_id = ?1 AND source = ?2", params![account, source], |r| r.get(0)).optional()
}

fn set_cursor(conn: &Connection, account: &str, source: &str, rowid: i64, error: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO cloud_event_cursors (account_id, source, last_rowid, updated_at, last_error) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(account_id, source) DO UPDATE SET last_rowid = excluded.last_rowid, updated_at = excluded.updated_at, last_error = excluded.last_error",
        params![account, source, rowid, chrono::Utc::now().to_rfc3339(), error],
    )?;
    Ok(())
}

fn note_error(conn: &Connection, account: &str, source: &str, error: &str) {
    let _ = conn.execute("UPDATE cloud_event_cursors SET last_error = ?3, updated_at = ?4 WHERE account_id = ?1 AND source = ?2", params![account, source, error, chrono::Utc::now().to_rfc3339()]);
}

fn ledger_end(conn: &Connection, account: &str, source: &str) -> rusqlite::Result<i64> {
    let sql = if source == SOURCE_BOT {
        "SELECT COALESCE(MAX(e.rowid), 0) FROM bot_events e JOIN agents a ON a.id = e.bot_id WHERE a.user_id = ?1"
    } else {
        "SELECT COALESCE(MAX(seq), 0) FROM runtime_user_events WHERE user_id = ?1"
    };
    conn.query_row(sql, params![account], |r| r.get(0))
}

/// Rows after `after` for `account`: (rows, highest rowid scanned, rows scanned).
pub fn read_rows(conn: &Connection, account: &str, source: &str, after: i64) -> rusqlite::Result<(Vec<LedgerRow>, i64, i64)> {
    let sql = if source == SOURCE_BOT {
        "SELECT e.rowid, e.id, e.event_type, e.bot_id, a.name, e.thread_id, t.title, e.payload, e.occurred_at, COALESCE(t.incognito, 0)
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.rowid > ?2 ORDER BY e.rowid LIMIT ?3"
    } else {
        "SELECT seq, id, event_type, NULL, NULL, thread_id, NULL, payload, occurred_at, 0
         FROM runtime_user_events WHERE user_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut last = after;
    let mut out = vec![];
    let rows = stmt.query_map(params![account, after, BATCH], |r| {
        Ok((
            LedgerRow {
                rowid: r.get(0)?,
                id: r.get(1)?,
                event_type: r.get(2)?,
                bot_id: r.get(3)?,
                bot_name: r.get(4)?,
                thread_id: r.get(5)?,
                thread_title: r.get(6)?,
                payload: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(Value::Null),
                occurred_at: r.get(8)?,
            },
            r.get::<_, i64>(9)? != 0,
        ))
    })?;
    let mut scanned = 0;
    for row in rows {
        let (row, incognito) = row?;
        scanned += 1;
        last = last.max(row.rowid);
        if !incognito {
            out.push(row);
        }
    }
    Ok((out, last, scanned))
}

// ---------------------------------------------------------------- delivery

/// Where batches go. The production sink is HTTP; tests record.
#[async_trait]
pub trait EventSink: Send + Sync {
    /// POST `body` to `url` with `headers`; the HTTP status, or an error when
    /// nothing answered.
    async fn post(&self, url: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String>;
}

pub struct HttpSink;

#[async_trait]
impl EventSink for HttpSink {
    async fn post(&self, url: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String> {
        let mut req = reqwest::Client::new().post(url).timeout(Duration::from_secs(20)).header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req.body(body).send().await.map(|r| r.status().as_u16()).map_err(|e| e.to_string())
    }
}

/// The headers one batch is sent with.
pub fn request_headers(token: &str, owner: &str, runtime_id: &str, body: &[u8]) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = crate::relay_auth::signed_headers(token, owner, "POST", EVENTS_PATH, body).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    h.push((crate::relay_auth::RUNTIME_ID_HEADER.into(), runtime_id.to_string()));
    h
}

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Events the cloud accepted.
    pub sent: usize,
    /// `true` when a source returned a full batch (more is waiting).
    pub more: bool,
}

/// One pass over both sources. `Ok` with nothing sent when the runtime isn't
/// paired (no device token, owner or runtime id: the cloud finds the device
/// by `x-allternit-runtime-id`). An `Err` leaves the cursor where it was.
pub async fn forward_once(db: &DbHandle, secret: &dyn RelaySecret, sink: &dyn EventSink, cloud: &str) -> Result<Pass, String> {
    let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
        return Ok(Pass::default());
    };
    let mut pass = Pass::default();
    for source in [SOURCE_BOT, SOURCE_USER] {
        let (db2, o) = (db.clone(), owner.clone());
        let read = tokio::task::spawn_blocking(move || -> rusqlite::Result<(Vec<Value>, i64, i64, bool)> {
            let conn = db2.connect()?;
            let after = match cursor(&conn, &o, source)? {
                Some(c) => c,
                None => {
                    // First run for this account: start at the end, no replay.
                    let end = ledger_end(&conn, &o, source)?;
                    set_cursor(&conn, &o, source, end, None)?;
                    end
                }
            };
            let (rows, last, scanned) = read_rows(&conn, &o, source, after)?;
            let full = scanned >= BATCH;
            Ok((rows.iter().filter_map(|r| envelope(source, r)).collect(), after, last, full))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let (events, after, last, full) = read;
        if last == after {
            continue;
        }
        if !events.is_empty() {
            let body = serde_json::to_vec(&json!({ "events": events })).map_err(|e| e.to_string())?;
            let headers = request_headers(&token, &owner, &runtime_id, &body);
            let outcome = match sink.post(&format!("{cloud}{EVENTS_PATH}"), headers, body).await {
                Ok(s) if (200..300).contains(&s) => Ok(()),
                Ok(s) => Err(format!("the cloud answered {s}")),
                Err(e) => Err(e),
            };
            if let Err(e) = outcome {
                let (db2, o, e2) = (db.clone(), owner.clone(), e.clone());
                let _ = tokio::task::spawn_blocking(move || db2.connect().map(|c| note_error(&c, &o, source, &e2))).await;
                return Err(format!("{source}: {e}"));
            }
            pass.sent += events.len();
        }
        let (db2, o) = (db.clone(), owner.clone());
        tokio::task::spawn_blocking(move || db2.connect().and_then(|c| set_cursor(&c, &o, source, last, None)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        pass.more |= full;
    }
    Ok(pass)
}

/// Next wait after a failure: doubles from [`POLL`] up to [`MAX_BACKOFF`].
pub fn backoff(prev: Duration) -> Duration {
    (prev.max(POLL) * 2).min(MAX_BACKOFF)
}

/// Background forwarder (inert until the runtime is paired).
pub async fn run(state: Arc<AppState>) {
    let secret = crate::relay_auth::process_secret();
    let mut wait = POLL;
    let mut failing = Duration::ZERO;
    loop {
        tokio::time::sleep(wait).await;
        match forward_once(&state.db, secret.as_ref(), &HttpSink, &crate::phone_sync::cloud_base()).await {
            Ok(p) => {
                if p.sent > 0 {
                    tracing::debug!(sent = p.sent, "runtime events forwarded to the cloud");
                }
                failing = Duration::ZERO;
                wait = if p.more { Duration::from_millis(200) } else { POLL };
            }
            Err(e) => {
                failing = backoff(failing);
                wait = failing;
                tracing::debug!(retry_in = ?wait, "runtime event forward: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Paired(Option<(&'static str, &'static str)>);
    impl RelaySecret for Paired {
        fn device_token(&self) -> Option<String> {
            self.0.map(|p| p.0.to_string())
        }
        fn paired_owner(&self) -> Option<String> {
            self.0.map(|p| p.1.to_string())
        }
        fn runtime_id(&self) -> Option<String> {
            self.0.map(|_| "rt_test".to_string())
        }
    }

    #[derive(Default)]
    struct Recorder {
        status: Mutex<Vec<Result<u16, String>>>,
        calls: Mutex<Vec<(String, Vec<(String, String)>, Value)>>,
    }
    impl Recorder {
        fn answering(s: Vec<Result<u16, String>>) -> Self {
            Self { status: Mutex::new(s), calls: Mutex::default() }
        }
        fn events(&self) -> Vec<Value> {
            self.calls.lock().unwrap().iter().flat_map(|c| c.2["events"].as_array().cloned().unwrap_or_default()).collect()
        }
    }
    #[async_trait]
    impl EventSink for Recorder {
        async fn post(&self, url: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String> {
            self.calls.lock().unwrap().push((url.to_string(), headers, serde_json::from_slice(&body).unwrap()));
            let mut s = self.status.lock().unwrap();
            if s.is_empty() { Ok(202) } else { s.remove(0) }
        }
    }

    const PAIRED: Paired = Paired(Some(("tok", "user-a")));

    async fn setup() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("allternit-fwd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        for (id, u) in [("bot-a", "user-a"), ("bot-b", "user-b")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, ?2, 'Ada', 'm', 'p', 1, '{}')", params![id, u]).unwrap();
        }
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES ('th-1','user-a','bot-a','Plans','idle','t','t','t')", []).unwrap();
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, incognito, last_activity_at, created_at, updated_at) VALUES ('th-x','user-a','bot-a','Secret','idle',1,'t','t','t')", []).unwrap();
        (state, dir)
    }

    fn put(c: &Connection, bot: &str, thread: &str, ty: &str, payload: Value) -> String {
        let seq: i64 = c.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM bot_events WHERE bot_id = ?1", params![bot], |r| r.get(0)).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO bot_events (id, bot_id, seq, event_type, actor_type, actor_id, payload, occurred_at, thread_id) VALUES (?1, ?2, ?3, ?4, 'user', 'x', ?5, '2026-10-05T10:00:00Z', ?6)",
            params![id, bot, seq, ty, payload.to_string(), thread],
        )
        .unwrap();
        id
    }

    /// Every ledger type written on this runtime → registry type, or ignored.
    /// A new ledger type must be added here (and to `classify` if it matters).
    #[test]
    fn mapping_table_is_exhaustive() {
        let table: &[(&str, Value, Option<&str>)] = &[
            ("channel.message.received", json!({ "text": "hi" }), Some("message.received")),
            ("channel.message.received", json!({ "own": true }), None),
            ("channel.message.sent", json!({}), None),
            ("channel.message.pending", json!({}), None),
            ("call.started", json!({}), None),
            ("call.transcript.delta", json!({}), None),
            ("call.summary", json!({}), None),
            ("call.ended", json!({ "reason": "hangup" }), Some("call.ended")),
            ("call.ended", json!({ "reason": "no_answer" }), Some("call.ended")),
            ("thread.created", json!({}), None),
            ("thread.started", json!({}), None),
            ("thread.blocked", json!({}), None),
            ("thread.needs_user", json!({}), Some("thread.needs_user")),
            ("thread.completed", json!({}), None),
            ("thread.failed", json!({}), None),
            ("thread.paused", json!({}), None),
            ("thread.queued", json!({}), None),
            ("thread.idle", json!({}), None),
            ("thread.status_changed", json!({}), None),
            ("approval.requested", json!({ "approvalId": "ap1" }), Some("approval.requested")),
            ("approval.resolved", json!({ "approvalId": "ap1" }), Some("approval.resolved")),
            ("task.waiting_for_approval", json!({}), None),
            ("task.resumed", json!({}), None),
            ("task.waiting_for_input", json!({}), None),
            ("run.started", json!({}), None),
            ("run.completed", json!({}), Some("agent.run.completed")),
            ("run.failed", json!({}), None),
            ("run.blocked", json!({}), None),
            ("routine.completed", json!({}), Some("agent.run.completed")),
            ("routine.failed", json!({}), None),
            ("vendor.ticket.created", json!({ "ticketId": "T-1" }), Some("vendor.ticket.created")),
            ("vendor.ticket.result", json!({}), None),
            ("inbox.changed", json!({}), None),
            ("inbox.item.created", json!({}), Some("inbox.item.created")),
            ("memory.promoted", json!({}), None),
            ("gateway.turn.blocked", json!({}), None),
            ("gateway.remote_thread.lost", json!({}), None),
            ("tool.noise", json!({}), None),
            ("subscription.login_needed", json!({}), Some("subscription.login_needed")),
            ("subscription.signed_in", json!({}), Some("subscription.signed_in")),
            ("subscription.task.needs_user", json!({}), Some("thread.needs_user")),
        ];
        for (ty, payload, want) in table {
            let got = classify(ty, payload).and_then(Kind::registry);
            assert_eq!(got, *want, "{ty} {payload}");
        }
        // The registry names this runtime emits are all in the agreed list.
        let registry = ["approval.requested", "approval.resolved", "agent.run.completed", "thread.needs_user", "message.received", "call.ended", "inbox.item.created", "vendor.ticket.created", "subscription.login_needed", "subscription.signed_in", "usage.threshold"];
        for (_, _, want) in table {
            if let Some(w) = want {
                assert!(registry.contains(w), "{w}");
            }
        }
    }

    #[test]
    fn approvals_share_one_id_across_stores() {
        let row = |src: &str| LedgerRow {
            rowid: 1,
            id: format!("{src}-row"),
            event_type: "approval.resolved".into(),
            bot_id: None,
            bot_name: None,
            thread_id: None,
            thread_title: None,
            payload: json!({ "approvalId": "ap-9", "decision": "approved", "secret": "x" }),
            occurred_at: "2026-10-05T10:00:00Z".into(),
        };
        let a = envelope(SOURCE_BOT, &row("b")).unwrap();
        let b = envelope(SOURCE_USER, &row("u")).unwrap();
        assert_eq!(a["id"], "rt:approval:ap-9:resolved");
        assert_eq!(a["id"], b["id"]);
        assert!(a["data"].get("secret").is_none(), "payload keys are allowlisted");
        assert_eq!(a["data"]["decision"], "approved");
    }

    #[tokio::test]
    async fn unpaired_runtime_is_a_no_op() {
        let (st, _d) = setup().await;
        put(&st.db.connect().unwrap(), "bot-a", "th-1", "thread.needs_user", json!({}));
        let sink = Recorder::default();
        let p = forward_once(&st.db, &Paired(None), &sink, "https://cloud").await.unwrap();
        assert_eq!(p, Pass::default());
        assert!(sink.calls.lock().unwrap().is_empty());
        // Paired but without a runtime id: the cloud couldn't find the key, so nothing goes.
        struct NoId;
        impl RelaySecret for NoId {
            fn device_token(&self) -> Option<String> { Some("tok".into()) }
            fn paired_owner(&self) -> Option<String> { Some("user-a".into()) }
        }
        assert_eq!(forward_once(&st.db, &NoId, &sink, "https://cloud").await.unwrap(), Pass::default());
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM cloud_event_cursors", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn forwards_signed_scoped_and_advances_only_on_success() {
        let (st, _d) = setup().await;
        let c = st.db.connect().unwrap();
        put(&c, "bot-a", "th-1", "thread.needs_user", json!({})); // before pairing: never replayed
        let sink = Recorder::answering(vec![Ok(503), Err("timeout".into())]);
        assert_eq!(forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.unwrap().sent, 0);

        let id = put(&c, "bot-a", "th-1", "channel.message.received", json!({ "text": "hello there", "from": "Sam", "token": "nope" }));
        put(&c, "bot-a", "th-1", "channel.message.sent", json!({ "text": "ignored" }));
        put(&c, "bot-a", "th-x", "thread.needs_user", json!({})); // incognito: stays local
        put(&c, "bot-b", "", "thread.needs_user", json!({})); // another account
        record_user_event(&c, "user-a", "subscription.login_needed", None, &json!({ "loginId": "l1", "provider": "chatgpt" }), Some("k1")).unwrap();

        // 503 then a network error: cursor stays, the same rows go again.
        assert!(forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.is_err());
        assert!(forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.is_err());
        let before = cursor(&c, "user-a", SOURCE_BOT).unwrap().unwrap();
        let err: Option<String> = c.query_row("SELECT last_error FROM cloud_event_cursors WHERE account_id='user-a' AND source=?1", params![SOURCE_BOT], |r| r.get(0)).unwrap();
        assert!(err.unwrap().contains("timeout"));

        let p = forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.unwrap();
        assert_eq!(p.sent, 2);
        assert!(cursor(&c, "user-a", SOURCE_BOT).unwrap().unwrap() > before);

        let calls = sink.calls.lock().unwrap().clone();
        let (url, headers, body) = calls[calls.len() - 2].clone();
        assert_eq!(url, "https://cloud/api/v1/runtime/events");
        let h = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert!(h("x-allternit-runtime-sig").unwrap().starts_with("v1="));
        assert_eq!(h("x-allternit-owner").unwrap(), "user-a");
        assert_eq!(h("x-allternit-runtime-id").unwrap(), "rt_test");
        assert!(h("authorization").is_none(), "the device token never leaves the runtime");
        // The signature verifies with the runtime's own relay check.
        let mut hm = axum::http::HeaderMap::new();
        for (k, v) in &headers {
            hm.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        let raw = serde_json::to_vec(&body).unwrap();
        let st_secret = crate::relay_auth::StaticRelaySecret { token: "tok".into(), owner: "user-a".into() };
        assert_eq!(crate::relay_auth::verify_relay(&st_secret, &hm, "POST", EVENTS_PATH, &raw, crate::relay_auth::unix_now()).unwrap(), "user-a");

        let ev = &body["events"][0];
        assert_eq!(ev["id"], format!("rt:be:{id}"));
        assert_eq!(ev["type"], "message.received");
        assert_eq!(ev["bot_id"], "bot-a");
        assert_eq!(ev["thread_id"], "th-1");
        assert_eq!(ev["data"]["text"], "hello there");
        assert_eq!(ev["data"]["threadTitle"], "Plans");
        assert!(ev["data"].get("token").is_none());
        let ue = &sink.events().into_iter().filter(|e| e["type"] == "subscription.login_needed").last().unwrap();
        assert!(ue["id"].as_str().unwrap().starts_with("rt:ue:"));
        assert!(ue.get("bot_id").is_none());

        // Nothing new: no post at all.
        let n = sink.calls.lock().unwrap().len();
        assert_eq!(forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.unwrap().sent, 0);
        assert_eq!(sink.calls.lock().unwrap().len(), n);
    }

    #[tokio::test]
    async fn resumes_from_the_persisted_cursor_after_restart() {
        let (st, dir) = setup().await;
        let c = st.db.connect().unwrap();
        let sink = Recorder::default();
        forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.unwrap();
        let first = put(&c, "bot-a", "th-1", "thread.needs_user", json!({}));
        forward_once(&st.db, &PAIRED, &sink, "https://cloud").await.unwrap();
        let second = put(&c, "bot-a", "th-1", "run.completed", json!({ "run_id": "r1" }));
        drop(c);
        drop(st);

        // A fresh process on the same database.
        let st2 = crate::test_helpers::app_state(&dir).await;
        let sink2 = Recorder::default();
        assert_eq!(forward_once(&st2.db, &PAIRED, &sink2, "https://cloud").await.unwrap().sent, 1);
        let ids: Vec<_> = sink2.events().iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, vec![format!("rt:be:{second}")]);
        assert_eq!(sink.events()[0]["id"], format!("rt:be:{first}"));
        assert_eq!(sink2.events()[0]["type"], "agent.run.completed");
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(Duration::ZERO), Duration::from_secs(10));
        assert_eq!(backoff(Duration::from_secs(10)), Duration::from_secs(20));
        assert_eq!(backoff(Duration::from_secs(400)), MAX_BACKOFF);
    }

    #[test]
    fn user_events_are_idempotent_per_key() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../migrations/V236__runtime_event_forwarder.sql")).unwrap();
        assert!(record_user_event(&c, "u", "subscription.signed_in", None, &json!({}), Some("k")).unwrap());
        assert!(!record_user_event(&c, "u", "subscription.signed_in", None, &json!({}), Some("k")).unwrap());
        assert!(record_user_event(&c, "u", "subscription.signed_in", None, &json!({}), None).unwrap());
        assert!(record_user_event(&c, "u", "subscription.signed_in", None, &json!({}), None).unwrap());
    }
}
