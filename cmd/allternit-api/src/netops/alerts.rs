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
//! * **Critical by SMS (D2, opt-in, off by default).** With the owner's
//!   `alerts.criticalSms` setting on, an item that opens or escalates to
//!   `critical` gets `sms_due = 1`; the sweep sends ONE text from the bot's own
//!   number through `phone_outbound` (so the consent gate applies), at most one
//!   per item per 30 minutes, then clears `sms_due`.
//! * **Daily digest.** From 09:00 local, one `info` `digest` item per owner lists
//!   every failed or missed run and every lane that went red in the last 24 h.
//!   Nothing to list means no digest. The previous digest is resolved.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
    "runtime_asleep", "loop_guard", "egress_held", "injection_flagged", "tool_changed", "peer_held", "digest",
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
                mark_sms_due(conn, &item.owner, &id, severity);
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
            mark_sms_due(conn, &item.owner, &id, severity);
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

/// Run an item's retry now. `{"routineId"}` runs the routine again, as "Run now" does;
/// `{"pinApprove": {serverId, tool, hash}}` re-pins an MCP tool and resolves the item. Errors are plain sentences for the person.
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
    if let Some(pin) = spec.get("pinApprove").filter(|p| p.is_object()) {
        let (Some(server), Some(tool), Some(hash)) = (pin["serverId"].as_str(), pin["tool"].as_str(), pin["hash"].as_str()) else {
            return Err((StatusCode::BAD_REQUEST, "this approval is incomplete; dismiss it and let the bot list its tools again".into()));
        };
        let conn = state.db.connect().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let desc: Option<String> = conn
            .query_row("SELECT detail_json FROM attention_items WHERE id = ?1 AND owner = ?2", params![id, owner], |r| r.get::<_, Option<String>>(0))
            .ok()
            .flatten()
            .and_then(|d| serde_json::from_str::<Value>(&d).ok())
            .and_then(|d| d["newDescription"].as_str().map(String::from));
        crate::mcp_server_routes::pin_tool(&conn, owner, server, tool, hash, desc.as_deref());
        return Ok(json!({ "status": "approved", "serverId": server, "tool": tool }));
    }
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
    Router::new()
        .route("/inbox/:id/retry", post(retry_route))
        .route("/inbox/:id/resolve", post(resolve_route))
        .route("/alerts/sms", get(sms_get).put(sms_put))
}

// ---------------------------------------------------------------- critical SMS

const SMS_KEY: &str = "alerts.criticalSms";
/// One text per item in this window, however often it re-escalates.
const SMS_MIN_GAP: Duration = Duration::minutes(30);

fn setting(conn: &Connection, owner: &str, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM runtime_settings WHERE user_id = ?1 AND key = ?2", params![owner, key], |r| r.get(0)).optional().ok().flatten()
}

fn put_setting(conn: &Connection, owner: &str, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO runtime_settings (user_id, key, value, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
        params![owner, key, value],
    );
}

/// Where the owner's critical texts go: `None` while the setting is off (the default).
/// The number is the one saved with the setting, else the owner's own phone in People.
pub fn sms_destination(conn: &Connection, owner: &str) -> Option<String> {
    let v: Value = serde_json::from_str(&setting(conn, owner, SMS_KEY)?).ok()?;
    if v["enabled"].as_bool() != Some(true) {
        return None;
    }
    v["phone"].as_str().filter(|p| !p.trim().is_empty()).map(str::to_string).or_else(|| {
        conn.query_row(
            "SELECT i.external_id FROM person_identities i JOIN people p ON p.id = i.person_id
             WHERE i.owner = ?1 AND i.kind = 'phone' AND p.name_source = 'owner' AND p.merged_into IS NULL ORDER BY i.created_at LIMIT 1",
            params![owner],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    })
}

fn mark_sms_due(conn: &Connection, owner: &str, id: &str, severity: &str) {
    if severity == "critical" && sms_destination(conn, owner).is_some() {
        let _ = conn.execute("UPDATE attention_items SET sms_due = 1 WHERE id = ?1 AND owner = ?2", params![id, owner]);
    }
}

/// A text the sweep should send now.
#[derive(Debug, Clone, PartialEq)]
pub struct SmsJob {
    pub id: String,
    pub owner: String,
    pub to: String,
    pub bot_id: Option<String>,
    pub text: String,
}

/// Items waiting for a critical text. Resolved items, owners who turned the
/// setting off, and items texted in the last 30 minutes are cleared, not sent.
pub fn sms_pending(conn: &Connection, now: DateTime<Utc>) -> Vec<SmsJob> {
    let rows: Vec<(String, String, String, Option<String>, Option<String>)> = (|| -> rusqlite::Result<_> {
        let mut st = conn.prepare("SELECT id, owner, summary, bot_id, resolved_at FROM attention_items WHERE sms_due = 1 LIMIT 50")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    })()
    .unwrap_or_default();
    let mut jobs = vec![];
    for (id, owner, summary, bot_id, resolved_at) in rows {
        let last = setting(conn, &owner, &format!("alerts.sms_sent:{id}")).and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(|t| t.with_timezone(&Utc));
        let to = sms_destination(conn, &owner).filter(|p| crate::netops::sms_verify::is_verified(conn, &owner, p));
        match to {
            Some(to) if resolved_at.is_none() && last.map_or(true, |t| now - t >= SMS_MIN_GAP) => {
                jobs.push(SmsJob { id, owner, to, bot_id, text: format!("Allternit: {summary} Open Allternit to fix it.") });
            }
            _ => {
                let _ = conn.execute("UPDATE attention_items SET sms_due = 0 WHERE id = ?1", params![id]);
            }
        }
    }
    jobs
}

/// Record that the text went out (or was refused for good): clears `sms_due` and starts the 30-minute gap.
pub fn sms_done(conn: &Connection, job: &SmsJob, now: DateTime<Utc>) {
    let _ = conn.execute("UPDATE attention_items SET sms_due = 0 WHERE id = ?1", params![job.id]);
    put_setting(conn, &job.owner, &format!("alerts.sms_sent:{}", job.id), &now.to_rfc3339());
}

/// Send each pending text from the bot's own number via the existing outbound SMS path.
async fn deliver_sms(state: &Arc<AppState>, now: DateTime<Utc>) {
    let jobs = match state.db.connect() {
        Ok(conn) => sms_pending(&conn, now),
        Err(_) => return,
    };
    for job in jobs {
        let sent = match crate::phone_outbound::pick_number(&state.db, &job.owner, job.bot_id.as_deref(), None)
            .or_else(|_| crate::phone_outbound::pick_number(&state.db, &job.owner, None, None))
        {
            Ok(n) => crate::phone_outbound::text(&state.db, &crate::phone_outbound::runtime(state), Arc::new(crate::channel_transports::ReqwestSend), &n, &job.to, &job.text).await,
            Err(e) => Err(e),
        };
        // A refusal (no consent, opted out, no number) is final for this open: the item stays in the inbox and the push.
        if let Err(e) = &sent {
            warn!(attention = %job.id, error = %e.sentence(), "critical text was not sent");
        }
        if let Ok(conn) = state.db.connect() {
            sms_done(&conn, &job, now);
        }
    }
}

async fn sms_get(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let Ok(conn) = state.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "database unavailable") };
    let v: Value = setting(&conn, &user.user_id, SMS_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    Json(json!({ "enabled": v["enabled"].as_bool() == Some(true), "phone": v["phone"].as_str().unwrap_or(""), "resolvedPhone": sms_destination_any(&conn, &user.user_id) })).into_response()
}

/// The number a text would go to, even while the setting is off (so the UI can show it).
fn sms_destination_any(conn: &Connection, owner: &str) -> Option<String> {
    let v: Value = setting(conn, owner, SMS_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    v["phone"].as_str().filter(|p| !p.trim().is_empty()).map(str::to_string).or_else(|| {
        conn.query_row(
            "SELECT i.external_id FROM person_identities i JOIN people p ON p.id = i.person_id
             WHERE i.owner = ?1 AND i.kind = 'phone' AND p.name_source = 'owner' AND p.merged_into IS NULL ORDER BY i.created_at LIMIT 1",
            params![owner],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    })
}

async fn sms_put(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<Value>) -> Response {
    let Some(enabled) = body["enabled"].as_bool() else { return err(StatusCode::BAD_REQUEST, "enabled is required") };
    let phone = body["phone"].as_str().unwrap_or("").trim().to_string();
    if !phone.is_empty() && !crate::channel_phone::is_e164(&phone) {
        return err(StatusCode::BAD_REQUEST, "the phone number must look like +14155550123");
    }
    let Ok(conn) = state.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "database unavailable") };
    if enabled && phone.is_empty() && sms_destination_any(&conn, &user.user_id).is_none() {
        return err(StatusCode::CONFLICT, "add your phone number first, then turn this on");
    }
    put_setting(&conn, &user.user_id, SMS_KEY, &json!({ "enabled": enabled, "phone": phone }).to_string());
    Json(json!({ "enabled": enabled, "phone": phone, "resolvedPhone": sms_destination_any(&conn, &user.user_id) })).into_response()
}

// ---------------------------------------------------------------- daily digest

/// Open today's digest for every owner who had a failed or missed run, or a lane
/// go red, in the last 24 h. Runs from 09:00 local; once a day per owner; skipped
/// when there is nothing to list. Returns how many digests it opened.
pub fn digest_sweep(conn: &Connection, now: DateTime<Utc>) -> usize {
    let local = now.with_timezone(&Local);
    if local.time() < NaiveTime::from_hms_opt(DAILY_HOUR, 0, 0).unwrap_or_default() {
        return 0;
    }
    let day = local.format("%Y-%m-%d").to_string();
    let since = (now - Duration::hours(24)).to_rfc3339();
    let rows: Vec<(String, String, String, String)> = (|| -> rusqlite::Result<_> {
        let mut st = conn.prepare(
            "SELECT owner, kind, summary, id FROM attention_items
             WHERE kind IN ('routine_failed', 'routine_missed', 'lane_down') AND opened_at >= ?1 ORDER BY opened_at",
        )?;
        let rows = st.query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    })()
    .unwrap_or_default();
    let mut owners: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    owners.sort();
    owners.dedup();
    let mut opened = 0;
    for owner in owners {
        let digest_id = format!("digest:{day}");
        let seen: bool = conn.query_row("SELECT 1 FROM attention_items WHERE id = ?1 AND owner = ?2", params![digest_id, owner], |_| Ok(())).optional().ok().flatten().is_some();
        if seen {
            continue;
        }
        let mine: Vec<&(String, String, String, String)> = rows.iter().filter(|r| r.0 == owner).collect();
        let runs = mine.iter().filter(|r| r.1 != "lane_down").count();
        let lanes = mine.iter().filter(|r| r.1 == "lane_down").count();
        let mut parts = vec![];
        if runs > 0 {
            parts.push(format!("{runs} failed or missed {}", if runs == 1 { "run" } else { "runs" }));
        }
        if lanes > 0 {
            parts.push(format!("{lanes} {} went down", if lanes == 1 { "connection" } else { "connections" }));
        }
        let old: Vec<String> = conn
            .prepare("SELECT id FROM attention_items WHERE owner = ?1 AND kind = 'digest' AND resolved_at IS NULL")
            .and_then(|mut st| st.query_map(params![owner], |r| r.get(0)).map(|rs| rs.filter_map(Result::ok).collect()))
            .unwrap_or_default();
        for id in old {
            resolve(conn, &owner, &id, "superseded");
        }
        let items: Vec<Value> = mine.iter().map(|r| json!({ "id": r.3, "kind": r.1, "summary": r.2 })).collect();
        if open(conn, &Item { owner: owner.clone(), kind: "digest".into(), severity: "info".into(), source: "digest".into(), source_id: day.clone(), summary: format!("Last 24 hours: {}.", parts.join(", ")), detail: json!({ "items": items }), ..Default::default() }) {
            opened += 1;
        }
    }
    opened
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
                digest_sweep(&conn, Utc::now());
            }
            deliver_sms(&state, Utc::now()).await;
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

    fn with_settings(c: &Connection) {
        c.execute_batch("CREATE TABLE runtime_settings (user_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (user_id, key));
            CREATE TABLE people (id TEXT PRIMARY KEY, owner TEXT, name_source TEXT, merged_into TEXT);
            CREATE TABLE person_identities (id TEXT PRIMARY KEY, owner TEXT, person_id TEXT, kind TEXT, external_id TEXT, created_at TEXT);").unwrap();
    }

    #[test]
    fn critical_sms_is_sent_once_per_item_and_only_when_on() {
        let c = conn();
        with_settings(&c);
        let now = Utc::now();
        // Off by default: nothing becomes due.
        open(&c, &item("critical"));
        assert!(sms_pending(&c, now).is_empty());
        resolve(&c, "u", "routine_failed:r1", "fixed");
        // On: a critical open is due once; the fake sender records it.
        put_setting(&c, "u", SMS_KEY, r#"{"enabled":true,"phone":"+14155550123"}"#);
        // On but not verified: the critical text is dropped, never sent.
        open(&c, &item("critical"));
        assert!(sms_pending(&c, now).is_empty(), "an unverified number is never texted");
        resolve(&c, "u", "routine_failed:r1", "fixed");
        // Verified: the next critical open is texted once.
        let code = crate::netops::sms_verify::begin(&c, "u", "+14155550123", now).unwrap();
        crate::netops::sms_verify::confirm(&c, "u", &code, now).unwrap();
        open(&c, &item("critical"));
        let mut sent = vec![];
        for job in sms_pending(&c, now) {
            sent.push(job.to.clone());
            sms_done(&c, &job, now);
        }
        assert_eq!(sent, vec!["+14155550123".to_string()]);
        assert!(sms_pending(&c, now + Duration::minutes(1)).is_empty(), "sent once");
        // A re-open and re-escalation inside 30 minutes is rate limited.
        open(&c, &item("warn"));
        open(&c, &item("critical"));
        assert!(sms_pending(&c, now + Duration::minutes(10)).is_empty());
        // A warn never texts.
        let mut w = item("warn");
        w.source_id = "r9".into();
        open(&c, &w);
        assert!(sms_pending(&c, now + Duration::hours(2)).is_empty());
    }

    #[test]
    fn digest_is_skipped_when_empty_and_resolves_the_previous_one() {
        let c = conn();
        let nine_thirty = Local.from_local_datetime(&Local::now().date_naive().and_hms_opt(9, 30, 0).unwrap()).earliest().unwrap().with_timezone(&Utc);
        assert_eq!(digest_sweep(&c, nine_thirty), 0, "nothing to list");
        assert_eq!(c.query_row::<i64, _, _>("SELECT COUNT(*) FROM attention_items WHERE kind = 'digest'", [], |r| r.get(0)).unwrap(), 0);
        open(&c, &item("warn"));
        let before_nine = Local.from_local_datetime(&Local::now().date_naive().and_hms_opt(8, 0, 0).unwrap()).earliest().unwrap().with_timezone(&Utc);
        assert_eq!(digest_sweep(&c, before_nine), 0, "not before 09:00");
        assert_eq!(digest_sweep(&c, nine_thirty), 1);
        assert_eq!(digest_sweep(&c, nine_thirty + Duration::minutes(5)), 0, "once a day");
        let (sev, renotify): (String, Option<String>) = c.query_row("SELECT severity, renotify_at FROM attention_items WHERE kind = 'digest'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((sev.as_str(), renotify), ("info", None));
    }
}
