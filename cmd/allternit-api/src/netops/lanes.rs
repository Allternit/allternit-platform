//! Lane health (contract C5): the lane status board (`GET /api/v1/lanes/status`)
//! and the per-channel-binding breaker.
//!
//! A lane is a path a bot's words travel: a vendor subscription account (health
//! comes from the Sessions gateway through `subscription_sync`, kept in
//! `lane_samples`, V259) or a channel binding (health comes from the last hour
//! of `channel_message_log`). Model-provider lanes and the runtime row are not
//! on the board yet.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::AppState;

/// Vendor lane ids are `vendor:<gateway login id>`.
pub const VENDOR_PREFIX: &str = "vendor:";
/// Samples older than this are dropped.
const KEEP_DAYS: i64 = 14;
/// A binding's breaker opens after this many failed posts in a row …
const BREAKER_FAILS: usize = 5;
/// … all within this window …
const BREAKER_WINDOW: Duration = Duration::minutes(10);
/// … and lets one post through again after this long.
const BREAKER_HALF_OPEN_AFTER: Duration = Duration::minutes(2);
/// Failed or unconfirmed share of the last hour that turns a channel red.
const RED_RATIO: f64 = 0.5;
/// … and amber.
const AMBER_RATIO: f64 = 0.2;
/// Fewer posts than this in the hour never turn a channel red (one flake is not an outage).
const MIN_POSTS: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breaker {
    Closed,
    Open,
    HalfOpen,
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// The breaker for one channel binding, worked out from its outbound posts: the
/// last [`BREAKER_FAILS`] settled posts all failed inside [`BREAKER_WINDOW`] →
/// open; open and the newest failure is [`BREAKER_HALF_OPEN_AFTER`] old → half-open
/// (one post may try; if it fails the newest failure is fresh and it opens again).
pub fn channel_breaker(conn: &Connection, binding_id: &str, now: DateTime<Utc>) -> Breaker {
    let Ok(mut st) = conn.prepare(
        "SELECT state, updated_at FROM channel_message_log
         WHERE binding_id = ?1 AND direction = 'outbound' AND state IN ('confirmed', 'failed', 'unconfirmed')
         ORDER BY updated_at DESC LIMIT ?2",
    ) else {
        return Breaker::Closed;
    };
    let rows: Vec<(String, Option<DateTime<Utc>>)> = st
        .query_map(params![binding_id, BREAKER_FAILS as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|it| it.filter_map(Result::ok).map(|(s, t)| (s, parse_ts(&t))).collect())
        .unwrap_or_default();
    if rows.len() < BREAKER_FAILS || rows.iter().any(|(s, _)| s != "failed") {
        return Breaker::Closed;
    }
    let (Some(newest), Some(oldest)) = (rows.first().and_then(|r| r.1), rows.last().and_then(|r| r.1)) else {
        return Breaker::Closed;
    };
    if now - newest > BREAKER_WINDOW {
        return Breaker::Closed; // a stale run of failures
    }
    if newest - oldest > BREAKER_WINDOW {
        return Breaker::Closed; // the failures were spread out, not a burst
    }
    if now - newest >= BREAKER_HALF_OPEN_AFTER {
        Breaker::HalfOpen
    } else {
        Breaker::Open
    }
}

/// Whether posts on `binding_id` may go out (the channel breaker is not open).
/// Half-open lets one post through to test the lane.
pub fn channel_lane_open(conn: &Connection, binding_id: &str) -> bool {
    channel_breaker(conn, binding_id, Utc::now()) != Breaker::Open
}

/// The bots whose replies travel over this gateway login, with their names.
pub(crate) fn bots_for_login(conn: &Connection, owner: &str, login: &str) -> Vec<(String, String)> {
    let Ok(mut st) = conn.prepare(
        "SELECT DISTINCT e.bot_id, COALESCE(a.name, '') FROM provider_account_bindings p
         JOIN bot_execution_bindings e ON e.account_binding_id = p.id AND e.owner = p.owner
         LEFT JOIN agents a ON a.id = e.bot_id
         WHERE p.owner = ?1 AND p.external_account_id = ?2 ORDER BY e.bot_id",
    ) else {
        return vec![];
    };
    st.query_map(params![owner, format!("subsfab:{login}")], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// A bot reached by phone or text: a number on this runtime points at it.
pub(crate) fn bot_has_phone(conn: &Connection, bot_id: &str) -> bool {
    conn.query_row("SELECT 1 FROM channel_phone_numbers WHERE bot_id = ?1 LIMIT 1", params![bot_id], |_| Ok(())).optional().ok().flatten().is_some()
}

/// The newest state recorded for a lane.
pub(crate) fn last_state(conn: &Connection, owner: &str, lane_id: &str) -> Option<String> {
    conn.query_row("SELECT state FROM lane_samples WHERE owner = ?1 AND lane_id = ?2 ORDER BY at DESC, rowid DESC LIMIT 1", params![owner, lane_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

/// Records a state change (no row when nothing changed) and prunes old samples.
/// Returns the previous state when the state changed (or `Some("")` on first sight).
pub(crate) fn record_sample(conn: &Connection, owner: &str, lane_id: &str, state: &str, reason: Option<&str>, label: &str) -> rusqlite::Result<Option<String>> {
    let prev = last_state(conn, owner, lane_id);
    if prev.as_deref() == Some(state) {
        return Ok(None);
    }
    let now = Utc::now();
    conn.execute(
        "INSERT INTO lane_samples (lane_id, owner, state, reason, label, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![lane_id, owner, state, reason, label, now.to_rfc3339()],
    )?;
    conn.execute("DELETE FROM lane_samples WHERE at < ?1", params![(now - Duration::days(KEEP_DAYS)).to_rfc3339()])?;
    Ok(Some(prev.unwrap_or_default()))
}

fn bots_json(bots: &[(String, String)]) -> Value {
    Value::Array(bots.iter().map(|(id, name)| json!({ "id": id, "name": if name.is_empty() { id } else { name } })).collect())
}

fn vendor_lanes(conn: &Connection, owner: &str) -> Vec<Value> {
    let Ok(mut st) = conn.prepare(
        "SELECT s.lane_id, s.state, s.reason, s.label, s.at FROM lane_samples s
         WHERE s.owner = ?1 AND s.lane_id LIKE 'vendor:%'
           AND s.rowid = (SELECT s2.rowid FROM lane_samples s2 WHERE s2.owner = s.owner AND s2.lane_id = s.lane_id ORDER BY s2.at DESC, s2.rowid DESC LIMIT 1)
         ORDER BY s.lane_id",
    ) else {
        return vec![];
    };
    let rows: Vec<(String, String, Option<String>, Option<String>, String)> = st
        .query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(|(lane_id, state, reason, label, since)| {
            let login = lane_id.trim_start_matches(VENDOR_PREFIX);
            let bots = bots_for_login(conn, owner, login);
            let action = (state != "green").then(|| json!({ "label": "Fix", "url": "/?settings=subscriptions" }));
            json!({
                "laneId": lane_id,
                "kind": "vendor_account",
                "label": label.unwrap_or_else(|| login.to_string()),
                "state": state,
                "since": since,
                "reason": reason,
                "affectedBots": bots_json(&bots),
                "action": action,
            })
        })
        .collect()
}

fn channel_lanes(conn: &Connection, owner: &str, now: DateTime<Utc>) -> Vec<Value> {
    let hour_ago = (now - Duration::hours(1)).to_rfc3339();
    let Ok(mut st) = conn.prepare(
        "SELECT b.id, b.provider, b.channel_name, b.thread_id, t.bot_id, COALESCE(a.name, ''),
                SUM(CASE WHEN l.state IN ('failed', 'unconfirmed') THEN 1 ELSE 0 END), COUNT(*), MIN(CASE WHEN l.state IN ('failed', 'unconfirmed') THEN l.updated_at END)
         FROM channel_conversation_bindings b
         JOIN channel_message_log l ON l.binding_id = b.id AND l.direction = 'outbound' AND l.state IN ('confirmed', 'failed', 'unconfirmed') AND l.updated_at >= ?2
         LEFT JOIN bot_threads t ON t.id = b.thread_id
         LEFT JOIN agents a ON a.id = t.bot_id
         WHERE b.owner = ?1 GROUP BY b.id ORDER BY b.id",
    ) else {
        return vec![];
    };
    type Row = (String, String, Option<String>, String, Option<String>, String, i64, i64, Option<String>);
    let rows: Vec<Row> = st
        .query_map(params![owner, hour_ago], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(|(id, provider, channel, _thread, bot, bot_name, bad, total, first_bad)| {
            let breaker = channel_breaker(conn, &id, now);
            let ratio = if total > 0 { bad as f64 / total as f64 } else { 0.0 };
            let (state, reason) = if breaker == Breaker::Open {
                ("red", Some("Sends are paused after repeated failures; one will be tried again shortly".to_string()))
            } else if total >= MIN_POSTS && ratio >= RED_RATIO {
                ("red", Some(format!("{bad} of the last {total} messages did not go through")))
            } else if bad > 0 && ratio >= AMBER_RATIO {
                ("amber", Some(format!("{bad} of the last {total} messages did not go through")))
            } else {
                ("green", None)
            };
            let name = channel.filter(|c| !c.is_empty()).map(|c| format!("{provider} · {c}")).unwrap_or(provider);
            let bots: Vec<(String, String)> = bot.into_iter().map(|b| (b, bot_name.clone())).collect();
            json!({
                "laneId": format!("channel:{id}"),
                "kind": "channel",
                "label": name,
                "state": state,
                "since": if state == "green" { Value::Null } else { json!(first_bad) },
                "reason": reason,
                "affectedBots": bots_json(&bots),
                "action": (state != "green").then(|| json!({ "label": "Open", "url": "/?settings=channels" })),
            })
        })
        .collect()
}

/// Every lane for one owner, worst first (C5).
pub fn lane_status(conn: &Connection, owner: &str, now: DateTime<Utc>) -> Vec<Value> {
    let mut lanes = vendor_lanes(conn, owner);
    lanes.extend(channel_lanes(conn, owner, now));
    let rank = |l: &Value| match l["state"].as_str() {
        Some("red") => 0,
        Some("amber") => 1,
        _ => 2,
    };
    lanes.sort_by_key(rank);
    lanes
}

async fn status_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Json<Value> {
    let db = state.db.clone();
    let owner = user.user_id;
    let lanes = tokio::task::spawn_blocking(move || db.connect().map(|c| lane_status(&c, &owner, Utc::now())).unwrap_or_default()).await.unwrap_or_default();
    Json(Value::Array(lanes))
}

/// `GET /lanes/status`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/lanes/status", get(status_route))
}

/// Lane samplers. Vendor lanes are sampled by `subscription_sync`'s pull; channel
/// lanes are computed on read. Nothing runs here.
pub fn spawn(_state: Arc<AppState>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../migrations/V259__lane_health.sql")).unwrap();
        c.execute_batch(
            "CREATE TABLE channel_message_log (id TEXT PRIMARY KEY, owner TEXT, binding_id TEXT, thread_id TEXT, direction TEXT, kind TEXT, state TEXT, updated_at TEXT);
             CREATE TABLE channel_conversation_bindings (id TEXT PRIMARY KEY, owner TEXT, thread_id TEXT, provider TEXT, channel_name TEXT);
             CREATE TABLE bot_threads (id TEXT PRIMARY KEY, bot_id TEXT);
             CREATE TABLE agents (id TEXT PRIMARY KEY, name TEXT);",
        )
        .unwrap();
        c
    }

    fn post(c: &Connection, n: u32, binding: &str, state: &str, at: DateTime<Utc>) {
        c.execute(
            "INSERT INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, state, updated_at) VALUES (?1, 'u', ?2, 't', 'outbound', 'message', ?3, ?4)",
            params![format!("m{n}-{binding}-{state}"), binding, state, at.to_rfc3339()],
        )
        .unwrap();
    }

    #[test]
    fn breaker_opens_on_five_fails_in_ten_minutes_and_half_opens_after_two() {
        let c = conn();
        let now = Utc::now();
        for i in 0..4 {
            post(&c, i, "b1", "failed", now - Duration::seconds(30 + i as i64));
        }
        assert_eq!(channel_breaker(&c, "b1", now), Breaker::Closed, "four fails is not enough");
        post(&c, 4, "b1", "failed", now - Duration::seconds(20));
        assert_eq!(channel_breaker(&c, "b1", now), Breaker::Open);
        assert!(!channel_lane_open(&c, "b1"));
        // two minutes after the newest failure one post may try again
        assert_eq!(channel_breaker(&c, "b1", now + Duration::seconds(120)), Breaker::HalfOpen);
        // a success closes it
        post(&c, 5, "b1", "confirmed", now + Duration::seconds(121));
        assert_eq!(channel_breaker(&c, "b1", now + Duration::seconds(122)), Breaker::Closed);
        // failures spread over more than ten minutes never open it
        for i in 0..5 {
            post(&c, i, "b2", "failed", now - Duration::minutes(3 * i as i64 + 1));
        }
        assert_eq!(channel_breaker(&c, "b2", now), Breaker::Closed);
        // another binding is unaffected
        assert!(channel_lane_open(&c, "b3"));
    }

    #[test]
    fn channel_lanes_go_amber_then_red_by_the_hours_failures() {
        let c = conn();
        let now = Utc::now();
        c.execute("INSERT INTO channel_conversation_bindings VALUES ('b1','u','t1','slack','general')", []).unwrap();
        c.execute("INSERT INTO bot_threads VALUES ('t1','bot-1')", []).unwrap();
        c.execute("INSERT INTO agents VALUES ('bot-1','Ada')", []).unwrap();
        for i in 0..4 {
            post(&c, i, "b1", "confirmed", now - Duration::minutes(20 + i as i64));
        }
        post(&c, 9, "b1", "failed", now - Duration::minutes(15));
        let lanes = lane_status(&c, "u", now);
        assert_eq!(lanes[0]["state"], "amber");
        assert_eq!(lanes[0]["label"], "slack · general");
        assert_eq!(lanes[0]["affectedBots"][0]["name"], "Ada");
        for i in 10..14 {
            post(&c, i, "b1", "failed", now - Duration::minutes(14));
        }
        assert_eq!(lane_status(&c, "u", now)[0]["state"], "red");
        assert!(lane_status(&c, "other", now).is_empty());
    }

    #[test]
    fn samples_record_only_changes() {
        let c = conn();
        assert_eq!(record_sample(&c, "u", "vendor:a", "green", None, "Claude").unwrap(), Some(String::new()));
        assert_eq!(record_sample(&c, "u", "vendor:a", "green", None, "Claude").unwrap(), None);
        assert_eq!(record_sample(&c, "u", "vendor:a", "red", Some("Log in"), "Claude").unwrap(), Some("green".to_string()));
        let lanes = lane_status(&c, "u", Utc::now());
        assert_eq!(lanes.len(), 1);
        assert_eq!((lanes[0]["state"].as_str(), lanes[0]["reason"].as_str()), (Some("red"), Some("Log in")));
    }
}
