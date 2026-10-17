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

use crate::runtime_events::{EventSink, HttpSink};
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
        "nextDue": due.into_iter().map(|(t, id, title)| json!({ "routineId": id, "title": title, "at": t.to_rfc3339() })).collect::<Vec<_>>(),
    })
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

/// The heartbeat loop (inert until the runtime is paired).
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            if let Err(e) = beat_once(&state, &HttpSink, &crate::phone_sync::cloud_base()).await {
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
}
