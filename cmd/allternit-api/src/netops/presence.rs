//! Runtime presence (contract C6): a 60 s heartbeat to the cloud
//! (`POST /api/v1/runtime-presence/heartbeat`, relay-signed like
//! `runtime_events`) carrying the routines due in the next 24 h, so the cloud
//! (which is awake when this computer is not) can tell the owner when a
//! scheduled run is missed because this computer is asleep.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{json, Value};

use crate::runtime_events::EventSink;
use crate::AppState;

pub const HEARTBEAT_PATH: &str = "/api/v1/runtime-presence/heartbeat";
const EVERY: Duration = Duration::from_secs(60);
const HORIZON_HOURS: i64 = 24;
const MAX_DUE: usize = 50;

/// `routines.next_run_at` is free text: RFC 3339 or SQLite `YYYY-MM-DD HH:MM:SS` (UTC).
fn parse_at(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok().map(|n| n.and_utc()))
}

/// The heartbeat body (`{runtimeId, nextDue: [{routineId, title, at}]}`):
/// the owner's active routines due within 24 h, soonest first. `runtimeId`
/// is filled by the caller (the cloud takes it from the signed header).
pub fn heartbeat_payload(conn: &rusqlite::Connection, owner: &str) -> Value {
    heartbeat_payload_at(conn, owner, Utc::now())
}

pub fn heartbeat_payload_at(conn: &rusqlite::Connection, owner: &str, now: DateTime<Utc>) -> Value {
    let horizon = now + chrono::Duration::hours(HORIZON_HOURS);
    let mut due: Vec<(DateTime<Utc>, String, String)> = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT id, name, next_run_at FROM routines WHERE user_id = ?1 AND status = 'active' AND next_run_at IS NOT NULL",
    ) {
        if let Ok(rows) = st.query_map([owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
            for (id, title, at) in rows.flatten() {
                // Overdue routines are included: that is the missed run the cloud watches for.
                if let Some(t) = parse_at(&at).filter(|t| *t <= horizon) {
                    due.push((t, id, title));
                }
            }
        }
    }
    due.sort_by_key(|d| d.0);
    due.truncate(MAX_DUE);
    json!({
        "failovers": bot_failovers(conn, owner),
        "nextDue": due.into_iter().map(|(t, id, title)| json!({ "routineId": id, "title": title, "at": t.to_rfc3339() })).collect::<Vec<_>>(),
    })
}

/// One `{botId, failoverRuntimeId}` per always-on bot that has a failover computer.
fn bot_failovers(conn: &rusqlite::Connection, owner: &str) -> Vec<Value> {
    let Ok(mut st) = conn.prepare(
        "SELECT id, json_extract(config, '$.availability.failoverRuntimeId') FROM agents
         WHERE user_id = ?1 AND json_valid(config) AND json_extract(config, '$.availability.alwaysOn') IN (1, 'true')
           AND COALESCE(json_extract(config, '$.availability.failoverRuntimeId'), '') <> ''
         ORDER BY created_at",
    ) else { return Vec::new() };
    let rows: Vec<(String, String)> = st
        .query_map([owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(|(bot, to)| json!({ "botId": bot, "failoverRuntimeId": to, "accounts": default_accounts(conn, owner, &bot) }))
        .collect()
}

/// The channel connections whose default bot is `bot`: their public addresses
/// (cloud routes, labelled with the account id) follow this bot when it fails over.
fn default_accounts(conn: &rusqlite::Connection, owner: &str, bot: &str) -> Vec<String> {
    conn.prepare("SELECT account_id FROM channel_account_bots WHERE owner = ?1 AND bot_id = ?2 AND is_default = 1 ORDER BY account_id LIMIT 32")
        .and_then(|mut st| st.query_map(rusqlite::params![owner, bot], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()))
        .unwrap_or_default()
}

/// Write the failovers the cloud reports (it carried this computer's messages
/// while it was offline) to each always-on bot: a `bot_failovers` row and a
/// `bot.failover` event on its standing threads. Idempotent per failover id.
pub fn record_failovers(db: &crate::db::DbHandle, owner: &str, reply: &Value) -> usize {
    let Ok(conn) = db.connect() else { return 0 };
    let mut n = 0;
    for f in reply["failovers"].as_array().into_iter().flatten() {
        let (Some(fid), Some(from), Some(to), Some(start)) = (f["id"].as_str(), f["from"].as_str(), f["to"].as_str(), f["startedAt"].as_str()) else { continue };
        let reason = f["reason"].as_str().unwrap_or("home runtime offline");
        let end = f["endedAt"].as_str();
        // A per-bot failover names its bot; an older cloud's does not (every always-on bot on that computer).
        let only_bot = f["botId"].as_str();
        let bots: Vec<String> = conn
            .prepare(
                "SELECT id FROM agents WHERE user_id = ?1 AND json_valid(config)
                   AND json_extract(config, '$.availability.alwaysOn') IN (1, 'true')
                   AND json_extract(config, '$.availability.failoverRuntimeId') = ?2
                   AND (?3 IS NULL OR id = ?3)",
            )
            .and_then(|mut st| st.query_map(rusqlite::params![owner, to, only_bot], |r| r.get(0)).map(|rows| rows.flatten().collect()))
            .unwrap_or_default();
        for bot in bots {
            let row_id = format!("{fid}:{bot}");
            let inserted = conn
                .execute(
                    "INSERT OR IGNORE INTO bot_failovers (id, bot_id, from_runtime, to_runtime, started_at, ended_at, reason) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    rusqlite::params![row_id, bot, from, to, start, end, reason],
                )
                .unwrap_or(0);
            if inserted == 0 {
                continue;
            }
            n += 1;
            let threads: Vec<String> = conn
                .prepare("SELECT id FROM bot_threads WHERE bot_id = ?1 AND kind = 'standing'")
                .and_then(|mut st| st.query_map([&bot], |r| r.get(0)).map(|rows| rows.flatten().collect()))
                .unwrap_or_default();
            for t in threads {
                let payload = json!({ "from": from, "to": to, "startedAt": start, "endedAt": end, "reason": reason });
                // The thread event rides the bot ledger, like every other thread event.
                let _ = crate::bot_event_routes::append_event(
                    db,
                    &bot,
                    &crate::bot_event_routes::AppendEventBody {
                        event_type: "bot.failover".into(),
                        actor: crate::bot_event_routes::ActorBody { r#type: "system".into(), id: "presence".into() },
                        payload,
                        occurred_at: None,
                        session_id: None,
                        goal_id: None,
                        wih_id: None,
                        task_id: None,
                        run_id: None,
                        idempotency_key: Some(format!("failover:{fid}:{t}")),
                    },
                    &Utc::now().to_rfc3339(),
                )
                .map(|_| {
                    let _ = conn.execute(
                        "UPDATE bot_events SET thread_id = ?1 WHERE bot_id = ?2 AND idempotency_key = ?3",
                        rusqlite::params![t, bot, format!("failover:{fid}:{t}")],
                    );
                });
            }
        }
    }
    n
}

/// Runtime side serves no routes; the heartbeat goes out, not in.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}

/// Send one heartbeat. `Ok(false)` = not paired (nothing to do).
pub async fn beat_once(state: &AppState, sink: &dyn EventSink, cloud: &str) -> Result<bool, String> {
    let secret = crate::relay_auth::process_secret();
    let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
        return Ok(false);
    };
    let db = state.db.clone();
    let o = owner.clone();
    let mut body = tokio::task::spawn_blocking(move || db.connect().map(|c| heartbeat_payload(&c, &o)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    body["runtimeId"] = json!(runtime_id);
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let headers = sign(&token, &owner, &runtime_id, &bytes);
    match sink.post(&format!("{cloud}{HEARTBEAT_PATH}"), headers, bytes).await {
        Ok(s) if (200..300).contains(&s) => Ok(true),
        Ok(s) => Err(format!("the cloud answered {s}")),
        Err(e) => Err(e),
    }
}

fn sign(token: &str, owner: &str, runtime_id: &str, body: &[u8]) -> Vec<(String, String)> {
    // Same relay scheme as the events forwarder, different path.
    let mut h: Vec<(String, String)> = crate::relay_auth::signed_headers(token, owner, "POST", HEARTBEAT_PATH, body)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    h.push((crate::relay_auth::RUNTIME_ID_HEADER.into(), runtime_id.to_string()));
    h
}

/// [`beat_once`] over HTTP that also reads the cloud's reply: the failovers
/// that carried this computer's messages while it was offline are written to
/// the always-on bots ([`record_failovers`]).
async fn beat_and_reconcile(state: &AppState, cloud: &str) -> Result<bool, String> {
    let secret = crate::relay_auth::process_secret();
    let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
        return Ok(false);
    };
    let db = state.db.clone();
    let o = owner.clone();
    let mut body = tokio::task::spawn_blocking(move || db.connect().map(|c| heartbeat_payload(&c, &o)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    body["runtimeId"] = json!(runtime_id);
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let mut req = reqwest::Client::new().post(format!("{cloud}{HEARTBEAT_PATH}")).timeout(Duration::from_secs(20)).header("content-type", "application/json");
    for (k, v) in sign(&token, &owner, &runtime_id, &bytes) {
        req = req.header(k, v);
    }
    let res = req.body(bytes).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("the cloud answered {}", res.status()));
    }
    if let Ok(reply) = res.json::<Value>().await {
        if reply["failovers"].as_array().is_some_and(|a| !a.is_empty()) {
            let db = state.db.clone();
            let _ = tokio::task::spawn_blocking(move || record_failovers(&db, &owner, &reply)).await;
        }
    }
    Ok(true)
}

/// The heartbeat loop (inert until the runtime is paired).
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            if let Err(e) = beat_and_reconcile(&state, &crate::phone_sync::cloud_base()).await {
                tracing::debug!("runtime presence heartbeat: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_lists_routines_due_within_24h_soonest_first() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE routines (id TEXT, user_id TEXT, name TEXT, status TEXT, next_run_at TEXT);
             INSERT INTO routines VALUES ('late','u','Late','active','2026-10-17 20:00:00');
             INSERT INTO routines VALUES ('soon','u','Morning briefing','active','2026-10-16T08:00:00Z');
             INSERT INTO routines VALUES ('far','u','Far','active','2026-10-20 08:00:00');
             INSERT INTO routines VALUES ('off','u','Paused','paused','2026-10-16 09:00:00');
             INSERT INTO routines VALUES ('theirs','v','Theirs','active','2026-10-16 09:00:00');",
        )
        .unwrap();
        let now = DateTime::parse_from_rfc3339("2026-10-16T07:00:00Z").unwrap().with_timezone(&Utc);
        let v = heartbeat_payload_at(&c, "u", now);
        let ids: Vec<&str> = v["nextDue"].as_array().unwrap().iter().map(|d| d["routineId"].as_str().unwrap()).collect();
        assert_eq!(ids, ["soon"]);
        assert_eq!(v["nextDue"][0]["title"], "Morning briefing");
    }

    #[test]
    fn payload_lists_one_failover_per_always_on_bot_with_its_default_channels() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            r#"CREATE TABLE routines (id TEXT, user_id TEXT, name TEXT, status TEXT, next_run_at TEXT);
             CREATE TABLE agents (id TEXT, user_id TEXT, config TEXT, created_at TEXT);
             CREATE TABLE channel_account_bots (account_id TEXT, bot_id TEXT, owner TEXT, is_default INTEGER);
             INSERT INTO agents VALUES ('b1','u','{"availability":{"alwaysOn":true,"failoverRuntimeId":"cloud1"}}','1');
             INSERT INTO agents VALUES ('b2','u','{"availability":{"alwaysOn":true,"failoverRuntimeId":"cloud2"}}','2');
             INSERT INTO agents VALUES ('b3','u','{"availability":{"alwaysOn":false,"failoverRuntimeId":"cloud1"}}','3');
             INSERT INTO agents VALUES ('b4','v','{"availability":{"alwaysOn":true,"failoverRuntimeId":"cloud9"}}','4');
             INSERT INTO channel_account_bots VALUES ('tg-1','b1','u',1);
             INSERT INTO channel_account_bots VALUES ('tg-2','b1','u',0);
             INSERT INTO channel_account_bots VALUES ('wa-1','b2','u',1);"#,
        )
        .unwrap();
        let v = heartbeat_payload_at(&c, "u", Utc::now());
        assert_eq!(
            v["failovers"],
            json!([
                { "botId": "b1", "failoverRuntimeId": "cloud1", "accounts": ["tg-1"] },
                { "botId": "b2", "failoverRuntimeId": "cloud2", "accounts": ["wa-1"] }
            ])
        );
    }
}
