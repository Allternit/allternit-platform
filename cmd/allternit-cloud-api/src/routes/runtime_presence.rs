//! Runtime presence (agent-network contract C6, WP4).
//!
//! * `POST /api/v1/runtime-presence/heartbeat`: runtime → cloud every 60 s,
//!   relay-signed exactly like `/api/v1/runtime/events`
//!   ([`super::runtime_events::authenticate_runtime`]). Body
//!   `{"runtimeId", "nextDue": [{"routineId","title","at"}]}` (≤ 50 entries).
//! * `GET /api/v1/runtime-presence`: signed-in owner →
//!   `[{runtimeId, name, online, lastSeen, wakeable, nextDue}]`.
//! * The sweeper ([`sweep`], once a minute) raises `attention.opened` through
//!   [`super::allternit_events::emit_user_event`] and resolves it
//!   (`attention.resolved`) when the runtime is back:
//!   (a) `missed_run`: offline > 10 min past a reported `nextDue`;
//!   (b) `queue_waiting`: inbound channel requests waiting > 10 min for an offline runtime;
//!   (c) `queue_gave_up`: the 24 h channel give-up (critical, raised by `channel_inbound`).

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{allternit_events, runtime_events};
use crate::{ApiError, ApiState};

pub const HEARTBEAT_PATH: &str = "/api/v1/runtime-presence/heartbeat";
/// A heartbeat this recent counts as online (three missed beats).
const ONLINE_WITHIN_SECS: i64 = 180;
/// How long past a due time or a waiting message before the owner is told.
const GRACE_MINS: i64 = 10;
const MAX_DUE: usize = 50;
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(HEARTBEAT_PATH, post(heartbeat))
        .route("/api/v1/runtime-presence", get(list))
}

fn clean_due(body: &Value) -> Value {
    let due: Vec<Value> = body["nextDue"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    let at = d["at"].as_str().filter(|s| DateTime::parse_from_rfc3339(s).is_ok())?;
                    let id = d["routineId"].as_str().filter(|s| !s.is_empty() && s.len() <= 128)?;
                    let title: String = d["title"].as_str().unwrap_or("A routine").chars().take(120).collect();
                    Some(json!({ "routineId": id, "title": title, "at": at }))
                })
                .take(MAX_DUE)
                .collect()
        })
        .unwrap_or_default();
    Value::Array(due)
}

async fn heartbeat(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > 64 * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({ "error": "too_large" }))).into_response();
    }
    let (_user, runtime_id) =
        match runtime_events::authenticate_runtime(&state.db, &headers, "POST", HEARTBEAT_PATH, &body, Utc::now().timestamp()).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let Ok(parsed) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_body" }))).into_response();
    };
    let failover = parsed["failoverRuntimeId"].as_str().filter(|s| !s.is_empty() && s.len() <= 128);
    let per_bot = clean_failovers(&parsed);
    let result = async {
        record_heartbeat(&state.db, &runtime_id, clean_due(&parsed)).await?;
        set_failover_runtime(&state.db, &runtime_id, failover).await?;
        set_bot_failovers(&state.db, &runtime_id, &per_bot).await?;
        // The roster is best effort: a failed write never fails the heartbeat.
        if let Err(e) = super::team::store_sessions(&state.db, &_user, &runtime_id, &parsed).await {
            tracing::warn!("team roster: storing sessions failed: {e}");
        }
        take_finished_failovers(&state.db, &runtime_id).await
    }
    .await;
    match result {
        Ok(failovers) => Json(json!({ "ok": true, "failovers": failovers })).into_response(),
        Err(e) => {
            tracing::error!("runtime presence heartbeat: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
        }
    }
}

/// Store a heartbeat and close whatever alerts it answers.
pub async fn record_heartbeat(db: &PgPool, runtime_id: &str, next_due: Value) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE runtime_devices SET last_heartbeat_at = now(), last_seen_at = now(), relay_disconnected_at = NULL, \
         next_due = $2, status = 'online' WHERE id = $1",
    )
    .bind(runtime_id)
    .bind(next_due)
    .execute(db)
    .await?;
    // The home runtime is back: any failover that was carrying its messages ends here.
    sqlx::query("UPDATE runtime_failovers SET ended_at = now() WHERE from_runtime = $1 AND ended_at IS NULL")
        .bind(runtime_id)
        .execute(db)
        .await?;
    resolve_all(db, runtime_id).await
}

/// The per-bot failovers a heartbeat reports: `[{botId, failoverRuntimeId}]`,
/// one per always-on bot, bounded and de-duplicated by bot.
pub fn clean_failovers(parsed: &Value) -> Value {
    let mut seen = HashSet::new();
    let list: Vec<Value> = parsed["failovers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let bot = f["botId"].as_str().filter(|s| !s.is_empty() && s.len() <= 128)?;
                    let to = f["failoverRuntimeId"].as_str().filter(|s| !s.is_empty() && s.len() <= 128)?;
                    let accounts: Vec<Value> = f["accounts"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.is_empty() && x.len() <= 256).take(32).map(Value::from).collect())
                        .unwrap_or_default();
                    seen.insert(bot.to_string()).then(|| json!({ "botId": bot, "failoverRuntimeId": to, "accounts": accounts }))
                })
                .take(64)
                .collect()
        })
        .unwrap_or_default();
    Value::Array(list)
}

/// Store the per-bot failover computers this runtime reported (replaces the list).
pub async fn set_bot_failovers(db: &PgPool, runtime_id: &str, list: &Value) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE runtime_devices SET bot_failovers = $2 WHERE id = $1")
        .bind(runtime_id)
        .bind(list)
        .execute(db)
        .await?;
    Ok(())
}

/// The always-on bot whose channel connection `account` (a route's label) is,
/// among the reported per-bot failovers: an address with no bot of its own
/// follows the default bot of its connection.
pub fn bot_for_account(list: &Value, account: &str) -> Option<String> {
    list.as_array()?
        .iter()
        .find(|f| f["accounts"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(account))))
        .and_then(|f| f["botId"].as_str())
        .map(str::to_string)
}

/// The failover computer for `bot` among the reported per-bot failovers.
pub fn failover_for_bot(list: &Value, bot: &str) -> Option<String> {
    list.as_array()?
        .iter()
        .find(|f| f["botId"].as_str() == Some(bot))
        .and_then(|f| f["failoverRuntimeId"].as_str())
        .map(str::to_string)
}

/// The cloud computer this runtime's always-on bots fail over to (`None` = none).
pub async fn set_failover_runtime(db: &PgPool, runtime_id: &str, to: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE runtime_devices SET failover_runtime_id = $2 WHERE id = $1")
        .bind(runtime_id)
        .bind(to)
        .execute(db)
        .await?;
    Ok(())
}

/// Finished failovers the runtime has not been told about yet, for it to write
/// to its bots' threads (`bot.failover`) and `bot_failovers`. Each is returned once.
pub async fn take_finished_failovers(db: &PgPool, runtime_id: &str) -> Result<Value, sqlx::Error> {
    let rows: Vec<(String, String, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "UPDATE runtime_failovers SET reported_at = now() WHERE from_runtime = $1 AND ended_at IS NOT NULL AND reported_at IS NULL \
         RETURNING id, to_runtime, reason, started_at, ended_at, bot_id",
    )
    .bind(runtime_id)
    .fetch_all(db)
    .await?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(id, to, reason, started, ended, bot)| json!({ "id": id, "from": runtime_id, "to": to, "reason": reason, "startedAt": started, "endedAt": ended, "botId": bot }))
            .collect(),
    ))
}

/// Where a queued message for `home` should go right now: `home` itself, or its
/// always-on failover computer while `home` is offline. The pure half of
/// [`delivery_runtime`].
pub fn pick_runtime(home: &str, online: bool, failover: Option<&str>, failover_wakeable: bool) -> String {
    match failover {
        Some(to) if !online && failover_wakeable && to != home => to.to_string(),
        _ => home.to_string(),
    }
}

/// The runtime a channel message for `home` is delivered to. `bot` is the bot
/// the route belongs to: only an always-on bot with a failover computer fails
/// over, a message for any other bot (or an unassigned route) stays queued for
/// home. Opens (once) the `runtime_failovers` row when it switches;
/// [`record_heartbeat`] closes it.
pub async fn delivery_runtime(db: &PgPool, user_id: &str, home: &str, bot: Option<&str>, account: Option<&str>, connected: &HashSet<String>, now: DateTime<Utc>) -> Result<String, sqlx::Error> {
    let row: Option<(Option<DateTime<Utc>>, Option<String>, Value)> = sqlx::query_as(
        "SELECT last_heartbeat_at, failover_runtime_id, bot_failovers FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(home)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    // Never beat = not a runtime that reports presence: keep today's behaviour.
    let Some((Some(beat), legacy, per_bot)) = row else { return Ok(home.to_string()) };
    // A route made before routes carried a bot follows its connection's default
    // bot (the runtime reports which connections each always-on bot answers by
    // default); with no per-bot list at all, the legacy single failover applies
    // (a runtime that has not updated).
    let account_bot = match bot {
        Some(_) => None,
        None => account.and_then(|a| bot_for_account(&per_bot, a)),
    };
    let bot = bot.or(account_bot.as_deref());
    let to = match bot {
        Some(b) => failover_for_bot(&per_bot, b),
        None => legacy.filter(|_| per_bot.as_array().is_none_or(|a| a.is_empty())),
    };
    let Some(to) = to else { return Ok(home.to_string()) };
    let online = is_online(connected.contains(home), Some(beat), now);
    if online {
        return Ok(home.to_string());
    }
    let wakeable = is_wakeable(db, user_id, &to).await?;
    let target = pick_runtime(home, online, Some(&to), wakeable);
    if target != home {
        let opened = sqlx::query(
            "INSERT INTO runtime_failovers (id, user_id, from_runtime, to_runtime, bot_id) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
        )
        .bind(format!("fo_{}", uuid::Uuid::new_v4().simple()))
        .bind(user_id)
        .bind(home)
        .bind(&target)
        .bind(bot)
        .execute(db)
        .await?;
        if opened.rows_affected() > 0 {
            tracing::info!(home, to = %target, "always-on failover: delivering queued messages to the failover computer");
        }
    }
    Ok(target)
}

/// A cloud computer of this owner (one that `connect_or_wake_runtime` can start).
pub async fn is_wakeable(db: &PgPool, user_id: &str, runtime_id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM provisioned_instances p JOIN runtime_devices d ON d.id = p.device_id \
         WHERE p.device_id = $1 AND d.user_id = $2 AND d.revoked_at IS NULL AND p.status <> 'deleted')",
    )
    .bind(runtime_id)
    .bind(user_id)
    .fetch_one(db)
    .await
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    name: Option<String>,
    last_heartbeat_at: Option<DateTime<Utc>>,
    last_seen_at: Option<DateTime<Utc>>,
    next_due: Value,
}

fn is_online(connected: bool, beat: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    connected || beat.is_some_and(|b| now - b < Duration::seconds(ONLINE_WITHIN_SECS))
}

async fn list(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, last_heartbeat_at, last_seen_at, next_due FROM runtime_devices \
         WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let connected = super::runtime_relay::connected_runtime_ids().await;
    let now = Utc::now();
    let mut wakeable = HashSet::new();
    for r in &rows {
        if is_wakeable(&state.db, &user.id, &r.id).await.unwrap_or(false) {
            wakeable.insert(r.id.clone());
        }
    }
    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let can_wake = wakeable.contains(&r.id);
            json!({
                "runtimeId": r.id,
                "name": r.name.unwrap_or_else(|| "Computer".into()),
                "online": is_online(connected.contains(&r.id), r.last_heartbeat_at, now),
                "lastSeen": r.last_heartbeat_at.max(r.last_seen_at),
                "wakeable": can_wake,
                "nextDue": r.next_due,
            })
        })
        .collect();
    Ok(Json(Value::Array(out)).into_response())
}

// ------------------------------------------------------------------ alerts

/// Open one alert (idempotent per runtime + kind) and announce it as `attention.opened`.
pub async fn open_alert(
    db: &PgPool,
    user_id: &str,
    runtime_id: &str,
    kind: &str,
    severity: &str,
    summary: &str,
) -> Result<bool, sqlx::Error> {
    let id = format!("pres_{}", uuid::Uuid::new_v4().simple());
    let inserted: Option<String> = sqlx::query_scalar(
        "INSERT INTO runtime_presence_alerts (id, runtime_id, user_id, kind, summary) VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (runtime_id, kind) WHERE resolved_at IS NULL DO NOTHING RETURNING id",
    )
    .bind(&id)
    .bind(runtime_id)
    .bind(user_id)
    .bind(kind)
    .bind(summary)
    .fetch_optional(db)
    .await?;
    let Some(id) = inserted else { return Ok(false) };
    allternit_events::emit_user_event(
        db,
        user_id,
        "attention.opened",
        &json!({ "attentionId": id, "severity": severity, "kind": kind, "summary": summary, "actionUrl": "/settings/computers" }),
        None,
        "cloud:presence",
        Some(&format!("opened:{id}")),
    )
    .await?;
    Ok(true)
}

/// Close this runtime's open alerts (optionally one kind) with `attention.resolved`.
async fn resolve_kinds(db: &PgPool, runtime_id: &str, kind: Option<&str>) -> Result<(), sqlx::Error> {
    let closed: Vec<(String, String, String, String)> = sqlx::query_as(
        "UPDATE runtime_presence_alerts SET resolved_at = now() WHERE runtime_id = $1 AND resolved_at IS NULL \
         AND ($2::text IS NULL OR kind = $2) RETURNING id, user_id, kind, summary",
    )
    .bind(runtime_id)
    .bind(kind)
    .fetch_all(db)
    .await?;
    for (id, user, kind, summary) in closed {
        allternit_events::emit_user_event(
            db,
            &user,
            "attention.resolved",
            &json!({ "attentionId": id, "kind": kind, "summary": summary }),
            None,
            "cloud:presence",
            Some(&format!("resolved:{id}")),
        )
        .await?;
    }
    Ok(())
}

async fn resolve_all(db: &PgPool, runtime_id: &str) -> Result<(), sqlx::Error> {
    resolve_kinds(db, runtime_id, None).await
}

/// One sweep. `connected` = runtimes with a live relay connection. Pure of
/// the clock and the relay hub so tests drive it.
pub async fn sweep(db: &PgPool, connected: &HashSet<String>, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    if let Err(e) = super::team::prune(db).await {
        tracing::warn!("team roster: prune failed: {e}");
    }
    let rows: Vec<(String, String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Value)> = sqlx::query_as(
        "SELECT id, user_id, last_heartbeat_at, relay_disconnected_at, next_due FROM runtime_devices \
         WHERE revoked_at IS NULL AND user_id IS NOT NULL AND last_heartbeat_at IS NOT NULL",
    )
    .fetch_all(db)
    .await?;
    for (id, user, beat, disconnected_at, next_due) in rows {
        if is_online(connected.contains(&id), beat, now) {
            resolve_all(db, &id).await?;
            continue;
        }
        if disconnected_at.is_none() {
            sqlx::query("UPDATE runtime_devices SET relay_disconnected_at = $2, status = 'offline' WHERE id = $1")
                .bind(&id)
                .bind(now)
                .execute(db)
                .await?;
        }
        let grace = now - Duration::minutes(GRACE_MINS);
        // (a) the first reported routine that came due and was never answered.
        let missed = next_due.as_array().into_iter().flatten().find_map(|d| {
            let at = DateTime::parse_from_rfc3339(d["at"].as_str()?).ok()?.with_timezone(&Utc);
            (at < grace).then(|| d["title"].as_str().unwrap_or("A routine").to_string())
        });
        if let Some(title) = missed {
            open_alert(db, &user, &id, "missed_run", "warn", &format!("{title} didn't run: your computer is asleep.")).await?;
        }
        // (b) messages waiting for this runtime.
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM channel_inbound_queue q JOIN channel_inbound_routes r ON r.id = q.route_id \
             WHERE r.runtime_id = $1 AND q.delivered_at IS NULL AND q.dead_at IS NULL AND q.received_at < $2",
        )
        .bind(&id)
        .bind(grace)
        .fetch_one(db)
        .await?;
        if waiting > 0 {
            let s = if waiting == 1 { "1 text is waiting for your computer.".to_string() } else { format!("{waiting} texts are waiting for your computer.") };
            open_alert(db, &user, &id, "queue_waiting", "warn", &s).await?;
        } else {
            resolve_kinds(db, &id, Some("queue_waiting")).await?;
        }
        if !next_due.as_array().into_iter().flatten().any(|d| {
            d["at"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).is_some_and(|t| t.with_timezone(&Utc) < grace)
        }) {
            resolve_kinds(db, &id, Some("missed_run")).await?;
        }
    }
    Ok(())
}

pub fn start_presence_sweeper(state: Arc<ApiState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            let connected = super::runtime_relay::connected_runtime_ids().await;
            if let Err(e) = sweep(&state.db, &connected, Utc::now()).await {
                tracing::warn!("runtime presence sweep: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_due_drops_bad_entries_and_caps() {
        let v = clean_due(&json!({ "nextDue": [
            { "routineId": "r1", "title": "Briefing", "at": "2026-10-16T08:00:00Z" },
            { "routineId": "", "title": "x", "at": "2026-10-16T08:00:00Z" },
            { "routineId": "r3", "title": "x", "at": "not a date" },
        ]}));
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["title"], "Briefing");
        assert_eq!(clean_due(&json!({})), json!([]));
    }

    #[test]
    fn online_when_connected_or_recent_beat() {
        let now = Utc::now();
        assert!(is_online(true, None, now));
        assert!(is_online(false, Some(now - Duration::seconds(60)), now));
        assert!(!is_online(false, Some(now - Duration::minutes(5)), now));
        assert!(!is_online(false, None, now));
    }

    use super::super::test_support::{events_backbone_schema, seed_runtime_device, test_pool};

    async fn schema() -> PgPool {
        let db = test_pool().await;
        events_backbone_schema(&db).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/090_runtime_presence.sql").replace("public.", "")).execute(&db).await.unwrap();
        // A heartbeat also closes failovers (091/092), so every test needs them.
        sqlx::raw_sql(&include_str!("../../migrations_pg/091_runtime_failover.sql").replace("public.", "")).execute(&db).await.unwrap();
        sqlx::raw_sql(&include_str!("../../migrations_pg/092_per_bot_failover.sql").replace("public.", "")).execute(&db).await.unwrap();
        seed_runtime_device(&db, "rt1", "u1").await;
        db
    }

    async fn events(db: &PgPool, name: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM platform_events WHERE user_id = 'u1' AND type = $1").bind(name).fetch_one(db).await.unwrap()
    }

    #[tokio::test]
    async fn missed_run_and_waiting_texts_open_then_resolve_when_back() {
        let db = schema().await;
        let now = Utc::now();
        let due = json!([{ "routineId": "r1", "title": "Morning briefing", "at": (now - Duration::minutes(30)).to_rfc3339() }]);
        sqlx::query("UPDATE runtime_devices SET last_heartbeat_at = $1, next_due = $2 WHERE id = 'rt1'")
            .bind(now - Duration::minutes(40)).bind(&due).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider) VALUES ('ro1','k','u1','rt1','sms')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO channel_inbound_queue (route_id, method, headers, body, received_at) VALUES ('ro1','POST','{}'::jsonb,'', $1)")
            .bind(now - Duration::minutes(20)).execute(&db).await.unwrap();

        let none = HashSet::new();
        sweep(&db, &none, now).await.unwrap();
        sweep(&db, &none, now).await.unwrap(); // idempotent
        assert_eq!(events(&db, "attention.opened").await, 2, "(a) missed run + (b) waiting text, once each");
        let summaries: Vec<String> = sqlx::query_scalar("SELECT summary FROM runtime_presence_alerts ORDER BY kind").fetch_all(&db).await.unwrap();
        assert_eq!(summaries, ["Morning briefing didn't run: your computer is asleep.", "1 text is waiting for your computer."]);

        // (c) the 24 h give-up is critical and also resolves when the runtime is back.
        assert!(open_alert(&db, "u1", "rt1", "queue_gave_up", "critical", "dropped").await.unwrap());
        assert_eq!(events(&db, "attention.opened").await, 3);

        record_heartbeat(&db, "rt1", json!([])).await.unwrap();
        assert_eq!(events(&db, "attention.resolved").await, 3);
        let open: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_presence_alerts WHERE resolved_at IS NULL").fetch_one(&db).await.unwrap();
        assert_eq!(open, 0);
    }

    #[test]
    fn failover_only_while_home_is_offline_and_target_is_a_cloud_computer() {
        assert_eq!(pick_runtime("mac", false, Some("cloud"), true), "cloud");
        assert_eq!(pick_runtime("mac", true, Some("cloud"), true), "mac", "home is back: switch back");
        assert_eq!(pick_runtime("mac", false, Some("cloud"), false), "mac", "not a cloud computer: nothing to wake");
        assert_eq!(pick_runtime("mac", false, None, true), "mac");
    }

    #[tokio::test]
    async fn failover_opens_once_and_closes_when_home_beats_again() {
        let db = schema().await;
        sqlx::query("CREATE TABLE IF NOT EXISTS provisioned_instances (device_id text, status text)").execute(&db).await.unwrap();
        seed_runtime_device(&db, "cloud1", "u1").await;
        sqlx::query("INSERT INTO provisioned_instances VALUES ('cloud1','stopped')").execute(&db).await.unwrap();
        let now = Utc::now();
        sqlx::query("UPDATE runtime_devices SET last_heartbeat_at = $1 WHERE id = 'rt1'").bind(now - Duration::minutes(30)).execute(&db).await.unwrap();
        set_bot_failovers(&db, "rt1", &json!([{ "botId": "b1", "failoverRuntimeId": "cloud1", "accounts": ["tg-acct-1"] }])).await.unwrap();
        let none = HashSet::new();
        // Per bot: a bot that is not always-on (not in the list) and an unassigned route stay queued for home.
        assert_eq!(delivery_runtime(&db, "u1", "rt1", Some("other"), None, &none, now).await.unwrap(), "rt1");
        assert_eq!(delivery_runtime(&db, "u1", "rt1", None, None, &none, now).await.unwrap(), "rt1");
        assert_eq!(delivery_runtime(&db, "u1", "rt1", Some("b1"), None, &none, now).await.unwrap(), "cloud1");
        assert_eq!(delivery_runtime(&db, "u1", "rt1", Some("b1"), None, &none, now).await.unwrap(), "cloud1");
        // An address made before routes carried a bot follows its connection's default bot.
        assert_eq!(delivery_runtime(&db, "u1", "rt1", None, Some("tg-acct-1"), &none, now).await.unwrap(), "cloud1");
        assert_eq!(delivery_runtime(&db, "u1", "rt1", None, Some("other-acct"), &none, now).await.unwrap(), "rt1");
        let open: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_failovers WHERE ended_at IS NULL").fetch_one(&db).await.unwrap();
        assert_eq!(open, 1);
        record_heartbeat(&db, "rt1", json!([])).await.unwrap();
        assert_eq!(delivery_runtime(&db, "u1", "rt1", Some("b1"), None, &none, now).await.unwrap(), "rt1", "switched back");
        let told = take_finished_failovers(&db, "rt1").await.unwrap();
        assert_eq!(told.as_array().unwrap().len(), 1);
        assert_eq!(take_finished_failovers(&db, "rt1").await.unwrap().as_array().unwrap().len(), 0, "reported once");
    }

    #[tokio::test]
    async fn online_runtime_raises_nothing_and_heartbeat_auth_is_signed() {
        let db = schema().await;
        let now = Utc::now();
        sqlx::query("UPDATE runtime_devices SET last_heartbeat_at = $1, next_due = $2 WHERE id = 'rt1'")
            .bind(now - Duration::seconds(30))
            .bind(json!([{ "routineId": "r1", "title": "t", "at": (now - Duration::minutes(30)).to_rfc3339() }]))
            .execute(&db).await.unwrap();
        sweep(&db, &HashSet::new(), now).await.unwrap();
        assert_eq!(events(&db, "attention.opened").await, 0);

        let key = super::super::runtime_pairing::sha256_hex(b"token-of-rt1");
        let body = br#"{"nextDue":[]}"#;
        let ts = now.timestamp();
        let mut h = HeaderMap::new();
        h.insert("x-allternit-runtime-id", "rt1".parse().unwrap());
        h.insert("x-allternit-runtime-ts", ts.to_string().parse().unwrap());
        h.insert("x-allternit-runtime-sig", super::super::runtime_relay::sign_runtime_request(&key, ts, "POST", HEARTBEAT_PATH, body).parse().unwrap());
        let ok = runtime_events::authenticate_runtime(&db, &h, "POST", HEARTBEAT_PATH, body, ts).await;
        assert_eq!(ok.unwrap(), ("u1".to_string(), "rt1".to_string()));
        h.insert("x-allternit-runtime-sig", "v1=bad".parse().unwrap());
        assert!(runtime_events::authenticate_runtime(&db, &h, "POST", HEARTBEAT_PATH, body, ts).await.is_err());
    }
}
