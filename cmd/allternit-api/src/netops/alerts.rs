//! Attention items (contracts C1 and C2): the durable "something needs fixing"
//! record. The server owns alerts, not a client window: every unhappy outcome
//! (a failed or missed routine, a routine with no bot, a lost run record …)
//! opens one row in `attention_items` and writes `attention.opened` to
//! `runtime_user_events`, which the forwarder pushes through the cloud.
//!
//! * **Idempotent.** The id is `<kind>:<source_id>`, so the same problem
//!   opened again bumps `last_seen_at` / `attempts` instead of adding a row.
//!   An open that raises the severity announces it again.
//! * **Re-notify.** A minute-by-minute sweep re-sends every unresolved `warn` /
//!   `critical` item one hour after it opened, then daily at 09:00 local time,
//!   until it is resolved.
//! * **Resolve.** [`resolve`] closes an item and writes `attention.resolved`
//!   (not pushed by default). The owner can also dismiss it from the inbox.
//! * **Retry.** `retry_json` says how to run it again; today `{"routineId"}`
//!   runs the routine now through the same path as "Run now".
//!
//! Critical items by SMS (decision D2) is not built: `attention_items.sms_due`
//! stays 0 and nothing here sends a text.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

/// Re-notify the first time this long after an item opens, then daily.
const FIRST_RENOTIFY: Duration = Duration::hours(1);
/// Local hour of the daily re-notify.
const DAILY_HOUR: u32 = 9;
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

pub const KINDS: &[&str] = &[
    "routine_failed", "routine_missed", "routine_needs_bot", "dead_letter", "lane_down",
    "runtime_asleep", "loop_guard", "egress_held", "injection_flagged", "tool_changed", "digest",
];

/// What to open. `kind` + `source_id` make the id.
#[derive(Debug, Clone, Default)]
pub struct Item {
    pub owner: String,
    pub kind: String,
    /// `info`, `warn` (default when empty) or `critical`.
    pub severity: String,
    pub bot_id: Option<String>,
    pub thread_id: Option<String>,
    /// The subsystem that raised it (`routine`, `lane`, `hop` …).
    pub source: String,
    pub source_id: String,
    /// One plain sentence; it is the push body and the inbox row.
    pub summary: String,
    pub detail: Value,
    /// C4 `{trust, sourceKind, sourceRef, excerpt, flags}`.
    pub provenance: Option<Value>,
    /// How to run it again, e.g. `{"routineId": "…"}`.
    pub retry: Option<Value>,
}

impl Item {
    pub fn id(&self) -> String {
        format!("{}:{}", self.kind, self.source_id)
    }
}

fn rank(severity: &str) -> u8 {
    match severity {
        "info" => 0,
        "critical" => 2,
        _ => 1,
    }
}

fn norm_severity(s: &str) -> &'static str {
    match s {
        "info" => "info",
        "critical" => "critical",
        _ => "warn",
    }
}

fn bot_name(conn: &Connection, bot_id: Option<&str>) -> Option<String> {
    let id = bot_id?;
    conn.query_row("SELECT name FROM agents WHERE id = ?1", params![id], |r| r.get(0)).optional().ok().flatten()
}

fn action_url(bot_id: Option<&str>, thread_id: Option<&str>) -> String {
    match (bot_id, thread_id) {
        (Some(b), Some(t)) => format!("/?allternit_thread={t}&bot={b}"),
        (Some(b), None) => format!("/?bot={b}"),
        _ => "/".to_string(),
    }
}

fn announce(conn: &Connection, owner: &str, event: &str, id: &str, kind: &str, severity: &str, summary: &str, bot_id: Option<&str>, thread_id: Option<&str>, key: &str) {
    let payload = json!({
        "attentionId": id,
        "kind": kind,
        "severity": severity,
        "summary": summary,
        "actionUrl": action_url(bot_id, thread_id),
        "botName": bot_name(conn, bot_id),
    });
    if let Err(e) = crate::runtime_events::record_user_event(conn, owner, event, thread_id, &payload, Some(key)) {
        warn!(attention = %id, error = %e, "could not write the {event} event");
    }
}

/// Open (or re-open) an attention item. Returns `true` when this call opened a
/// new problem (no unresolved row before). Idempotent.
pub fn open(conn: &Connection, item: &Item) -> bool {
    let id = item.id();
    let severity = norm_severity(&item.severity);
    let now = Utc::now();
    let detail = if item.detail.is_null() { "{}".to_string() } else { item.detail.to_string() };
    let existing: Option<(Option<String>, String)> = conn
        .query_row("SELECT resolved_at, severity FROM attention_items WHERE id = ?1 AND owner = ?2", params![id, item.owner], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .unwrap_or(None);
    match existing {
        Some((None, old_severity)) => {
            let escalated = rank(severity) > rank(&old_severity);
            let level = if escalated { severity } else { old_severity.as_str() };
            let _ = conn.execute(
                "UPDATE attention_items SET last_seen_at = ?3, attempts = attempts + 1, summary = ?4, detail_json = ?5, severity = ?6,
                     retry_json = COALESCE(?7, retry_json), provenance_json = COALESCE(?8, provenance_json)
                 WHERE id = ?1 AND owner = ?2",
                params![id, item.owner, now.to_rfc3339(), item.summary, detail, level, item.retry.as_ref().map(Value::to_string), item.provenance.as_ref().map(Value::to_string)],
            );
            if escalated {
                announce(conn, &item.owner, "attention.opened", &id, &item.kind, severity, &item.summary, item.bot_id.as_deref(), item.thread_id.as_deref(), &format!("attention:{id}:escalated:{severity}:{}", now.timestamp()));
            }
            false
        }
        _ => {
            let renotify = (severity != "info").then(|| (now + FIRST_RENOTIFY).to_rfc3339());
            let written = conn.execute(
                "INSERT INTO attention_items (id, owner, kind, severity, bot_id, thread_id, source, source_id, summary, detail_json,
                     provenance_json, retry_json, attempts, opened_at, last_seen_at, renotify_at, resolved_at, resolution, sms_due)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?13, ?14, NULL, NULL, 0)
                 ON CONFLICT(id) DO UPDATE SET owner = ?2, severity = ?4, bot_id = ?5, thread_id = ?6, summary = ?9, detail_json = ?10,
                     provenance_json = ?11, retry_json = ?12, attempts = 1, opened_at = ?13, last_seen_at = ?13, renotify_at = ?14,
                     resolved_at = NULL, resolution = NULL, sms_due = 0",
                params![
                    id, item.owner, item.kind, severity, item.bot_id, item.thread_id, item.source, item.source_id, item.summary, detail,
                    item.provenance.as_ref().map(Value::to_string), item.retry.as_ref().map(Value::to_string), now.to_rfc3339(), renotify
                ],
            );
            if let Err(e) = written {
                warn!(attention = %id, error = %e, "could not open the attention item");
                return false;
            }
            announce(conn, &item.owner, "attention.opened", &id, &item.kind, severity, &item.summary, item.bot_id.as_deref(), item.thread_id.as_deref(), &format!("attention:{id}:opened:{}", now.to_rfc3339()));
            true
        }
    }
}

/// Close an item. `how` is `fixed` (the cause went away, e.g. the next run
/// succeeded), `dismissed` (the owner closed it) or `retried`. Returns whether
/// an open item was closed.
pub fn resolve(conn: &Connection, owner: &str, id: &str, how: &str) -> bool {
    let now = Utc::now().to_rfc3339();
    let row: Option<(String, String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT kind, severity, summary, bot_id, thread_id FROM attention_items WHERE id = ?1 AND owner = ?2 AND resolved_at IS NULL",
            params![id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .unwrap_or(None);
    let Some((kind, severity, summary, bot_id, thread_id)) = row else { return false };
    let closed = conn
        .execute(
            "UPDATE attention_items SET resolved_at = ?3, resolution = ?4, renotify_at = NULL WHERE id = ?1 AND owner = ?2 AND resolved_at IS NULL",
            params![id, owner, now, how],
        )
        .unwrap_or(0);
    if closed == 0 {
        return false;
    }
    announce(conn, owner, "attention.resolved", id, &kind, &severity, &summary, bot_id.as_deref(), thread_id.as_deref(), &format!("attention:{id}:resolved:{now}"));
    true
}

/// Close every open item of `kind` for one source (`resolve` by parts).
pub fn resolve_source(conn: &Connection, owner: &str, kind: &str, source_id: &str, how: &str) -> bool {
    resolve(conn, owner, &format!("{kind}:{source_id}"), how)
}

/// Run an item's retry now. Today that is `{"routineId"}` → the routine runs
/// again, as "Run now" does. Errors are plain sentences for the person.
pub async fn retry(state: &Arc<AppState>, owner: &str, id: &str) -> Result<Value, (StatusCode, String)> {
    let retry_json: Option<Option<String>> = state
        .db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT retry_json FROM attention_items WHERE id = ?1 AND owner = ?2 AND resolved_at IS NULL", params![id, owner], |r| r.get(0)).optional().ok().flatten());
    let Some(retry_json) = retry_json else {
        return Err((StatusCode::NOT_FOUND, "this problem is already fixed".into()));
    };
    let spec: Value = retry_json.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let Some(routine_id) = spec["routineId"].as_str() else {
        return Err((StatusCode::BAD_REQUEST, "this problem cannot be retried; fix the cause, then dismiss it".into()));
    };
    let routine = crate::routine_local_scheduler::get_routine(&state.db, routine_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter(|r| r.user_id == owner)
        .ok_or((StatusCode::NOT_FOUND, "that routine no longer exists".to_string()))?;
    if routine.agent_id.as_deref().map_or(true, |a| a.trim().is_empty()) {
        return Err((StatusCode::CONFLICT, "assign this routine to a bot first".into()));
    }
    let guard = crate::routine_local_scheduler::FlightGuard::acquire(routine_id)
        .ok_or((StatusCode::CONFLICT, "this routine is already running".to_string()))?;
    let (state, owner) = (state.clone(), owner.to_string());
    tokio::spawn(async move {
        let _guard = guard;
        let driver = crate::routine_local_scheduler::GizziDriver { state: state.clone() };
        crate::routine_local_scheduler::execute(&state.db, &driver, &routine, crate::routine_local_scheduler::Trigger::Manual { user_id: owner }).await;
    });
    Ok(json!({ "status": "running", "routineId": routine_id }))
}

/// The next 09:00 in this runtime's local zone strictly after `now`.
pub fn next_daily(now: DateTime<Utc>) -> DateTime<Utc> {
    let local = now.with_timezone(&Local);
    let nine = NaiveTime::from_hms_opt(DAILY_HOUR, 0, 0).unwrap_or_default();
    let mut day = local.date_naive();
    for _ in 0..3 {
        if let Some(t) = Local.from_local_datetime(&day.and_time(nine)).earliest() {
            if t.with_timezone(&Utc) > now {
                return t.with_timezone(&Utc);
            }
        }
        day = day.succ_opt().unwrap_or(day);
    }
    now + Duration::days(1)
}

/// Re-send every unresolved `warn` / `critical` item that is due. Returns how many.
pub fn sweep(conn: &Connection, now: DateTime<Utc>) -> usize {
    let due: Vec<(String, String, String, String, String, String, Option<String>, Option<String>, String)> = (|| -> rusqlite::Result<_> {
        let mut st = conn.prepare(
            "SELECT id, owner, kind, severity, summary, opened_at, bot_id, thread_id, renotify_at FROM attention_items
             WHERE resolved_at IS NULL AND renotify_at IS NOT NULL AND renotify_at <= ?1 AND severity != 'info' LIMIT 100",
        )?;
        let rows = st.query_map(params![now.to_rfc3339()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    })()
    .unwrap_or_default();
    let mut sent = 0;
    for (id, owner, kind, severity, summary, _opened, bot_id, thread_id, renotify_at) in due {
        // The cursor moves first (compare-and-set), so two sweeps never both send.
        let next = next_daily(now).to_rfc3339();
        let moved = conn
            .execute("UPDATE attention_items SET renotify_at = ?2 WHERE id = ?1 AND renotify_at = ?3 AND resolved_at IS NULL", params![id, next, renotify_at])
            .unwrap_or(0);
        if moved == 1 {
            announce(conn, &owner, "attention.opened", &id, &kind, &severity, &summary, bot_id.as_deref(), thread_id.as_deref(), &format!("attention:{id}:renotify:{renotify_at}"));
            sent += 1;
        }
    }
    sent
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg, "code": if status == StatusCode::NOT_FOUND { "NOT_FOUND" } else { "ATTENTION_ERROR" } }))).into_response()
}

/// The attention id behind an inbox id (`attention:<kind>:<source>`).
fn attention_id(inbox_id: &str) -> Option<&str> {
    inbox_id.strip_prefix("attention:")
}

async fn retry_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(inbox_id): Path<String>) -> Response {
    let Some(id) = attention_id(&inbox_id) else { return err(StatusCode::BAD_REQUEST, "only problems can be retried") };
    match retry(&state, &user.user_id, id).await {
        Ok(v) => Json(json!({ "id": inbox_id, "result": v })).into_response(),
        Err((s, m)) => err(s, &m),
    }
}

async fn resolve_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(inbox_id): Path<String>) -> Response {
    let Some(id) = attention_id(&inbox_id) else { return err(StatusCode::BAD_REQUEST, "only problems can be resolved") };
    let Ok(conn) = state.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "database unavailable") };
    if resolve(&conn, &user.user_id, id, "dismissed") {
        Json(json!({ "id": inbox_id, "state": "resolved" })).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "this problem is already fixed")
    }
}

/// `POST /inbox/:id/retry` and `/inbox/:id/resolve` (siblings of approve / dismiss / snooze).
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/inbox/:id/retry", post(retry_route)).route("/inbox/:id/resolve", post(resolve_route))
}

/// The re-notify sweep, once a minute.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Ok(conn) = state.db.connect() {
                let n = sweep(&conn, Utc::now());
                if n > 0 {
                    tracing::info!(n, "re-notified unresolved attention items");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../migrations/V258__attention_items.sql").split("-- Why a run").next().unwrap()).unwrap();
        c.execute_batch("CREATE TABLE runtime_user_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT, user_id TEXT, event_type TEXT, thread_id TEXT, payload TEXT, idempotency_key TEXT, occurred_at TEXT, UNIQUE(user_id, idempotency_key)); CREATE TABLE agents (id TEXT PRIMARY KEY, name TEXT);").unwrap();
        c
    }

    fn item(sev: &str) -> Item {
        Item { owner: "u".into(), kind: "routine_failed".into(), severity: sev.into(), source: "routine".into(), source_id: "r1".into(), summary: "Briefing failed".into(), ..Default::default() }
    }

    fn events(c: &Connection, ty: &str) -> i64 {
        c.query_row("SELECT COUNT(*) FROM runtime_user_events WHERE event_type = ?1", params![ty], |r| r.get(0)).unwrap()
    }

    #[test]
    fn open_is_idempotent_and_resolve_reopens() {
        let c = conn();
        assert!(open(&c, &item("warn")));
        assert!(!open(&c, &item("warn")));
        let (n, attempts): (i64, i64) = c.query_row("SELECT COUNT(*), MAX(attempts) FROM attention_items", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((n, attempts), (1, 2));
        assert_eq!(events(&c, "attention.opened"), 1);
        assert!(resolve(&c, "u", "routine_failed:r1", "fixed"));
        assert!(!resolve(&c, "u", "routine_failed:r1", "fixed"));
        assert_eq!(events(&c, "attention.resolved"), 1);
        assert!(open(&c, &item("warn")), "a resolved problem opens again");
        assert_eq!(events(&c, "attention.opened"), 2);
        // Someone else cannot close it.
        assert!(!resolve(&c, "other", "routine_failed:r1", "dismissed"));
    }

    #[test]
    fn escalation_announces_again() {
        let c = conn();
        open(&c, &item("warn"));
        open(&c, &item("critical"));
        assert_eq!(events(&c, "attention.opened"), 2);
        let sev: String = c.query_row("SELECT severity FROM attention_items", [], |r| r.get(0)).unwrap();
        assert_eq!(sev, "critical");
        open(&c, &item("warn"));
        let sev: String = c.query_row("SELECT severity FROM attention_items", [], |r| r.get(0)).unwrap();
        assert_eq!(sev, "critical", "a later warn does not lower it");
    }

    #[test]
    fn sweep_renotifies_at_one_hour_then_daily_until_resolved() {
        let c = conn();
        open(&c, &item("warn"));
        let mut info = item("info");
        info.source_id = "r2".into();
        open(&c, &info);
        let now = Utc::now();
        assert_eq!(sweep(&c, now + Duration::minutes(30)), 0);
        assert_eq!(sweep(&c, now + Duration::minutes(61)), 1, "info items are never re-sent");
        assert_eq!(sweep(&c, now + Duration::minutes(62)), 0, "once per due time");
        let next: String = c.query_row("SELECT renotify_at FROM attention_items WHERE id = 'routine_failed:r1'", [], |r| r.get(0)).unwrap();
        let next = DateTime::parse_from_rfc3339(&next).unwrap().with_timezone(&Utc);
        assert!(next > now + Duration::minutes(62) && next <= now + Duration::hours(25));
        assert_eq!(next.with_timezone(&Local).format("%H:%M").to_string(), "09:00");
        assert_eq!(sweep(&c, next), 1);
        assert_eq!(events(&c, "attention.opened"), 4, "two opens, then two re-sends");
        resolve(&c, "u", "routine_failed:r1", "fixed");
        assert_eq!(sweep(&c, next + Duration::days(2)), 0);
    }
}
