//! Notifications: the one push sink on the event backbone, and the owner's
//! per-event-type preferences. Migration `060_notification_preferences.sql`.
//!
//! ## The sink
//!
//! Every user event on the backbone (`platform_events`, `subject = 'user'`,
//! written by [`allternit_events::emit_user_event`] for runtime-forwarded and
//! cloud-raised events alike) is looked at once by [`sink_once`]: it claims
//! the event in `notification_push_log` (so restarts and replicas never push
//! twice), checks the owner's preference for that event type, builds plain
//! copy and a deep link ([`message_for`]) and sends it with
//! [`web_push::send_to_user`]. Events older than [`STALE_AFTER_HOURS`] at the
//! producer (a laptop forwarding its backlog after a night offline) are not
//! pushed. The sink only runs when the VAPID env is set ([`spawn_push_sink`]).
//!
//! ## Direct pushes that stay direct
//!
//! * **In-app call rings** (`inapp_calls::notify_ring`) and their **missed
//!   call** follow-up: a ring has a 45 s life and must leave at once with high
//!   urgency, and the missed-call push reuses the ring's tag to replace it. In-app
//!   calls are cloud-only (no runtime ledger row), so the backbone carries no
//!   event for them. The missed-call push follows the `call.ended` preference.
//! * **In-app messages** between invite-linked people (`inapp_calls::notify_message`):
//!   cloud-only too, not a backbone event. They follow the `message.received`
//!   preference.
//!
//! ## Moved onto the sink
//!
//! * **Channel messages relayed through a channel address**
//!   (`channel_inbound` → `web_push::notify_channel_message`). The runtime
//!   records the same message as `channel.message.received` and forwards it as
//!   `message.received`, so when that runtime forwards events
//!   ([`runtime_forwards_events`]) the direct push is skipped and the sink
//!   sends it, with the sender and text. A runtime that doesn't forward yet
//!   (older build) keeps the direct push; the sink skips a `message.received`
//!   that lands within [`DIRECT_DEDUPE_SECS`] of a direct channel push, so the
//!   switch-over never notifies twice.
//!
//! Coding-session permission asks (`approval.requested` with
//! `source = "gizzi-permission"`) are pushed by the remote-control lane
//! (`services/remote-control-push`) and are not pushed again here.
//!
//! ## Preferences
//!
//! - `GET /api/v1/notifications/preferences` → `{pushConfigured, preferences: [{event, title, description, enabled, default}]}`
//! - `PUT /api/v1/notifications/preferences` `{preferences: {"<event>": true|false|null}}` → the same shape
//!   (`null` = back to the default). 400 `unknown_event` / `bad_value`.
//!
//! Clerk session or `compute`-scoped API key. Defaults keep what the app did
//! before the sink: messages and calls on, plus approvals, threads that need
//! you, sign-in needed and usage alerts; finished runs and other informational
//! events off ([`DEFAULTS`]).
//!
//! ## Quiet hours
//!
//! Migration `102_notification_quiet_hours.sql`.
//!
//! - `GET /api/v1/notifications/quiet-hours` → `{enabled, start: "HH:MM", end: "HH:MM", timeZone, allowCalls, letThrough}`
//! - `PUT /api/v1/notifications/quiet-hours` with any subset of those keys → the same shape. 400 `bad_value`
//!   (bad time, unknown zone, wrong type, unknown key, empty body).
//!
//! - `POST /api/v1/notifications/ack?t=<token>` (no auth; the token is the secret) → 204: the person opened a
//!   paged push, so it stops repeating. Sent by the service worker on click.
//!
//! No row = off, 22:00 to 07:00 UTC, calls ring, urgent alerts get through. The window is in the person's own zone (so it follows
//! daylight saving time), may wrap midnight, and `start == end` means no quiet hours.
//!
//! While a person is in quiet hours the sink and the direct pushes (in-app messages, relayed channel
//! messages, missed calls) don't send: [`hold_if_quiet`] records the push in `notification_held` (the sink
//! also marks the event `held` in `notification_push_log`). In-app call rings still ring when `allowCalls`
//! is on; when it's off the ring is silent and the missed call that follows is held.
//!
//! Every push has a [`Level`] ([`level`]): urgent (critical problems, anything a bot marked
//! `urgency: "urgent"`), needs you (approvals, threads waiting on the person, sign-in needed), normal
//! (messages, calls, other problems) or low. `letThrough` (`urgent` default, `needs_you`, `none`) says which
//! levels still go out during quiet hours. An urgent push that goes out during quiet hours pages: it is
//! recorded in `notification_repeat` (migration `107_quiet_hours_levels.sql`) with an ack link in its data,
//! and [`repeat_once`] sends it again every [`REPEAT_EVERY_MINUTES`] until it is opened or resolves (same
//! tag), at most [`MAX_REPEATS`] times. About once a minute the sink loop runs [`summarize_once`]: for every person with held pushes
//! who is no longer in quiet hours it claims them all in one `UPDATE … RETURNING` (so a restart or a second
//! replica never sends the summary twice) and sends one push, "While you were away" with counts by kind
//! ([`summary_body`]), opening `/`, tag `quiet-summary`. Nothing is re-sent one by one, so held pushes older
//! than [`STALE_AFTER_HOURS`] are counted like the rest.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use super::allternit_events::{EventType, REGISTRY};
use super::web_push::{self, PushMessage, PushTransport, Vapid};
use crate::ApiState;

/// Push on or off when the person never changed it, per event type. A type
/// missing here is off.
pub const DEFAULTS: &[(&str, bool)] = &[
    ("approval.requested", true),
    ("approval.resolved", false),
    ("agent.run.completed", false),
    ("thread.needs_user", true),
    // The app already pushed channel and in-app messages before the sink.
    ("message.received", true),
    ("call.ended", true),
    ("inbox.item.created", false),
    ("vendor.ticket.created", false),
    ("subscription.login_needed", true),
    ("subscription.signed_in", false),
    ("usage.threshold", true),
    // A failed or missed routine, a bot with no one to run it: the server pushes it and sends it again until it is fixed.
    ("attention.opened", true),
    ("attention.resolved", false),
];

/// How much a push matters during quiet hours. Ordered: a higher level gets through more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Low,
    Normal,
    NeedsYou,
    Urgent,
}

/// The level of one backbone event (or a direct push, which passes its own).
pub fn level(event: &str, data: &Value) -> Level {
    if s(data, &["urgency"]) == Some("urgent") {
        return Level::Urgent;
    }
    match event {
        "attention.opened" => match s(data, &["severity"]) {
            Some("critical") => Level::Urgent,
            Some("info") => Level::Low,
            _ => Level::Normal,
        },
        "approval.requested" | "thread.needs_user" | "subscription.login_needed" => Level::NeedsYou,
        "message.received" | "call.ended" => Level::Normal,
        _ => Level::Low,
    }
}

/// Resend an unopened urgent push sent in quiet hours this often ...
pub const REPEAT_EVERY_MINUTES: i32 = 5;
/// ... this many more times.
pub const MAX_REPEATS: i32 = 3;

/// Only look at events stored this recently (the sink polls every few seconds).
const SCAN_WINDOW_MINUTES: i64 = 10;
/// Don't push an event that happened at its producer longer ago than this.
pub const STALE_AFTER_HOURS: i64 = 12;
/// A `message.received` this soon after a direct channel push for the same person is the same message.
pub const DIRECT_DEDUPE_SECS: i64 = 120;
const POLL: Duration = Duration::from_secs(3);
const BATCH: i64 = 200;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/notifications/preferences", get(get_route).put(put_route))
        .route("/api/v1/notifications/quiet-hours", get(get_quiet_route).put(put_quiet_route))
        .route("/api/v1/notifications/ack", axum::routing::post(ack_route))
}

/// Event types a person can get pushed: every user-subject event in the registry.
pub fn pushable() -> impl Iterator<Item = &'static EventType> {
    REGISTRY.iter().filter(|e| e.agents || e.bot)
}

pub fn default_enabled(event: &str) -> bool {
    DEFAULTS.iter().find(|(n, _)| *n == event).is_some_and(|(_, on)| *on)
}

/// Whether `user` wants pushes for `event`. A read error falls back to the default.
pub async fn enabled(db: &PgPool, user: &str, event: &str) -> bool {
    match sqlx::query_scalar::<_, bool>("SELECT enabled FROM notification_preferences WHERE user_id = $1 AND event_type = $2").bind(user).bind(event).fetch_optional(db).await {
        Ok(Some(on)) => on,
        Ok(None) => default_enabled(event),
        Err(error) => {
            tracing::warn!("notifications: reading preference failed: {error}");
            default_enabled(event)
        }
    }
}

/// The preferences list the app renders as switches, in registry order.
pub async fn preferences(db: &PgPool, user: &str) -> Result<Value, sqlx::Error> {
    let rows: Vec<(String, bool)> = sqlx::query_as("SELECT event_type, enabled FROM notification_preferences WHERE user_id = $1").bind(user).fetch_all(db).await?;
    let list: Vec<Value> = pushable()
        .map(|e| {
            let default = default_enabled(e.name);
            let enabled = rows.iter().find(|(n, _)| n == e.name).map(|(_, on)| *on).unwrap_or(default);
            json!({ "event": e.name, "title": e.title, "description": e.description, "enabled": enabled, "default": default })
        })
        .collect();
    Ok(json!({ "pushConfigured": Vapid::from_env().is_some(), "preferences": list }))
}

#[derive(Debug, PartialEq)]
pub enum PrefError {
    Bad(&'static str),
    Db(String),
}

/// Apply `{"<event>": true | false | null}` for `user`, all or nothing.
pub async fn set_preferences(db: &PgPool, user: &str, changes: &Map<String, Value>) -> Result<(), PrefError> {
    if changes.is_empty() || changes.len() > REGISTRY.len() {
        return Err(PrefError::Bad("bad_value"));
    }
    for (event, value) in changes {
        if !pushable().any(|e| e.name == event) {
            return Err(PrefError::Bad("unknown_event"));
        }
        if !(value.is_boolean() || value.is_null()) {
            return Err(PrefError::Bad("bad_value"));
        }
    }
    let db_err = |e: sqlx::Error| PrefError::Db(e.to_string());
    let mut tx = db.begin().await.map_err(db_err)?;
    for (event, value) in changes {
        match value.as_bool() {
            Some(on) => {
                sqlx::query(
                    "INSERT INTO notification_preferences (user_id, event_type, enabled, updated_at) VALUES ($1, $2, $3, now()) \
                     ON CONFLICT (user_id, event_type) DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now()",
                )
                .bind(user)
                .bind(event)
                .bind(on)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            None => {
                sqlx::query("DELETE FROM notification_preferences WHERE user_id = $1 AND event_type = $2").bind(user).bind(event).execute(&mut *tx).await.map_err(db_err)?;
            }
        }
    }
    tx.commit().await.map_err(db_err)
}

async fn get_route(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match preferences(&state.db, &user).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

async fn put_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Option<Json<Value>>) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let Some(changes) = body.as_ref().and_then(|b| b.0.get("preferences")).and_then(Value::as_object) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_value" }))).into_response();
    };
    match set_preferences(&state.db, &user, changes).await {
        Ok(()) => match preferences(&state.db, &user).await {
            Ok(v) => Json(v).into_response(),
            Err(e) => internal(&e.to_string()),
        },
        Err(PrefError::Bad(code)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response(),
        Err(PrefError::Db(e)) => internal(&e),
    }
}

fn internal(error: &str) -> Response {
    tracing::error!("notifications: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
}

// ---------------------------------------------------------------------------
// Quiet hours
// ---------------------------------------------------------------------------

/// One person's quiet hours (`notification_quiet_hours`). Minutes after local midnight in `time_zone`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietHours {
    pub enabled: bool,
    pub start_min: i32,
    pub end_min: i32,
    pub time_zone: String,
    pub allow_calls: bool,
    /// The lowest level that still goes out during quiet hours; `None` holds everything.
    pub let_through: Option<Level>,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self { enabled: false, start_min: 22 * 60, end_min: 7 * 60, time_zone: "UTC".into(), allow_calls: true, let_through: Some(Level::Urgent) }
    }
}

/// `letThrough` on the wire and in `notification_quiet_hours.let_through`.
fn let_through_str(l: Option<Level>) -> &'static str {
    match l {
        Some(Level::Urgent) => "urgent",
        Some(_) => "needs_you",
        None => "none",
    }
}

fn parse_let_through(text: &str) -> Option<Option<Level>> {
    match text {
        "urgent" => Some(Some(Level::Urgent)),
        "needs_you" => Some(Some(Level::NeedsYou)),
        "none" => Some(None),
        _ => None,
    }
}

/// Whether `minute` (after local midnight) falls in `[start, end)`. Wraps midnight when `start > end`;
/// `start == end` is no window at all.
pub fn in_window(start_min: i32, end_min: i32, minute: i32) -> bool {
    if start_min == end_min {
        false
    } else if start_min < end_min {
        (start_min..end_min).contains(&minute)
    } else {
        minute >= start_min || minute < end_min
    }
}

impl QuietHours {
    /// In quiet hours at `now`: local wall-clock time in the person's zone, so daylight saving time moves
    /// the window with the clock. An unknown zone (never stored; validated on PUT) reads as UTC.
    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        use chrono::Timelike;
        if !self.enabled {
            return false;
        }
        let tz: chrono_tz::Tz = self.time_zone.parse().unwrap_or(chrono_tz::UTC);
        let local = now.with_timezone(&tz);
        in_window(self.start_min, self.end_min, (local.hour() * 60 + local.minute()) as i32)
    }

    pub fn json(&self) -> Value {
        json!({ "enabled": self.enabled, "start": hhmm(self.start_min), "end": hhmm(self.end_min), "timeZone": self.time_zone, "allowCalls": self.allow_calls, "letThrough": let_through_str(self.let_through) })
    }

    /// Apply a PUT body (any subset of the response keys). `Err("bad_value")` on anything invalid.
    pub fn apply(&mut self, changes: &Map<String, Value>) -> Result<(), &'static str> {
        if changes.is_empty() {
            return Err("bad_value");
        }
        for (key, value) in changes {
            match key.as_str() {
                "enabled" => self.enabled = value.as_bool().ok_or("bad_value")?,
                "allowCalls" => self.allow_calls = value.as_bool().ok_or("bad_value")?,
                "letThrough" => self.let_through = value.as_str().and_then(parse_let_through).ok_or("bad_value")?,
                "start" => self.start_min = value.as_str().and_then(parse_hhmm).ok_or("bad_value")?,
                "end" => self.end_min = value.as_str().and_then(parse_hhmm).ok_or("bad_value")?,
                "timeZone" => {
                    let zone = value.as_str().map(str::trim).ok_or("bad_value")?;
                    let tz: chrono_tz::Tz = zone.parse().map_err(|_| "bad_value")?;
                    self.time_zone = tz.name().to_string();
                }
                _ => return Err("bad_value"),
            }
        }
        Ok(())
    }

    /// Whether a push at `level` is held at `now`.
    pub fn holds_at(&self, level: Level, now: DateTime<Utc>) -> bool {
        self.active_at(now) && self.let_through.is_none_or(|min| level < min)
    }
}

fn hhmm(minutes: i32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// `"HH:MM"` (24-hour, two digits each) → minutes after midnight.
pub fn parse_hhmm(text: &str) -> Option<i32> {
    let (h, m) = text.trim().split_once(':')?;
    if h.len() != 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// `user`'s quiet hours, or the defaults when they never set them.
pub async fn quiet_hours(db: &PgPool, user: &str) -> Result<QuietHours, sqlx::Error> {
    let row: Option<(bool, i32, i32, String, bool, String)> = sqlx::query_as("SELECT enabled, start_min, end_min, time_zone, allow_calls, let_through FROM notification_quiet_hours WHERE user_id = $1").bind(user).fetch_optional(db).await?;
    Ok(row
        .map(|(enabled, start_min, end_min, time_zone, allow_calls, let_through)| QuietHours {
            enabled,
            start_min,
            end_min,
            time_zone,
            allow_calls,
            let_through: parse_let_through(&let_through).unwrap_or(Some(Level::Urgent)),
        })
        .unwrap_or_default())
}

pub async fn set_quiet_hours(db: &PgPool, user: &str, q: &QuietHours) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO notification_quiet_hours (user_id, enabled, start_min, end_min, time_zone, allow_calls, let_through, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
         ON CONFLICT (user_id) DO UPDATE SET enabled = EXCLUDED.enabled, start_min = EXCLUDED.start_min, end_min = EXCLUDED.end_min, \
         time_zone = EXCLUDED.time_zone, allow_calls = EXCLUDED.allow_calls, let_through = EXCLUDED.let_through, updated_at = now()",
    )
    .bind(user)
    .bind(q.enabled)
    .bind(q.start_min)
    .bind(q.end_min)
    .bind(&q.time_zone)
    .bind(q.allow_calls)
    .bind(let_through_str(q.let_through))
    .execute(db)
    .await
    .map(|_| ())
}

/// Whether `user` is in quiet hours now. A read error (for example before migration 102) is "not quiet",
/// so pushes keep working.
pub async fn is_quiet(db: &PgPool, user: &str) -> bool {
    match quiet_hours(db, user).await {
        Ok(q) => q.active_at(Utc::now()),
        Err(error) => {
            tracing::debug!("notifications: reading quiet hours failed: {error}");
            false
        }
    }
}

/// True when an in-app call ring for `user` should stay silent now (quiet hours with calls off).
pub async fn ring_held(db: &PgPool, user: &str) -> bool {
    match quiet_hours(db, user).await {
        Ok(q) => !q.allow_calls && q.active_at(Utc::now()),
        Err(_) => false,
    }
}

/// When `user` is in quiet hours and `level` is below what they let through, record `msg` as held
/// instead of sending it and return true. `event_id` is the backbone event for the sink (held once,
/// however often it's looked at); `None` for a direct push. The caller then doesn't send.
pub async fn hold_if_quiet(db: &PgPool, user: &str, event: &str, msg: &PushMessage, level: Level, severity: Option<&str>, event_id: Option<&str>) -> bool {
    let holds = match quiet_hours(db, user).await {
        Ok(q) => q.holds_at(level, Utc::now()),
        Err(error) => {
            tracing::debug!("notifications: reading quiet hours failed: {error}");
            false
        }
    };
    if !holds {
        return false;
    }
    let held = sqlx::query("INSERT INTO notification_held (user_id, event_type, tag, severity, source, event_id) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (event_id) DO NOTHING")
        .bind(user)
        .bind(event)
        .bind(&msg.tag)
        .bind(severity)
        .bind(if event_id.is_some() { "sink" } else { "direct" })
        .bind(event_id)
        .execute(db)
        .await;
    match held {
        Ok(_) => true,
        Err(error) => {
            // Better a push in quiet hours than a lost one.
            tracing::warn!("notifications: holding a push failed, sending it: {error}");
            false
        }
    }
}

/// An urgent push about to go out during `user`'s quiet hours pages: record it for [`repeat_once`] and put
/// its ack link in the data, so opening it on any device stops the repeats. Anything else is returned as is.
pub async fn page_if_urgent(db: &PgPool, user: &str, level: Level, mut msg: PushMessage) -> PushMessage {
    if level != Level::Urgent || !is_quiet(db, user).await {
        return msg;
    }
    // A newer page for the same thing replaces the older one.
    let _ = ack_tag(db, user, &msg.tag).await;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let ack = format!("{}/api/v1/notifications/ack?t={token}", super::channel_inbound::public_base());
    if let Some(data) = msg.data.as_object_mut() {
        data.insert("ack".into(), json!(ack));
    } else {
        msg.data = json!({ "ack": ack });
    }
    let recorded = sqlx::query(
        "INSERT INTO notification_repeat (user_id, tag, kind, title, body, url, data, ack_token, next_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(mins => $9))",
    )
    .bind(user)
    .bind(&msg.tag)
    .bind(msg.kind)
    .bind(&msg.title)
    .bind(&msg.body)
    .bind(&msg.url)
    .bind(&msg.data)
    .bind(&token)
    .bind(REPEAT_EVERY_MINUTES)
    .execute(db)
    .await;
    if let Err(error) = recorded {
        tracing::warn!("notifications: recording a page failed (it goes out once): {error}");
    }
    msg
}

/// Stop repeating `user`'s open pages with `tag` (the item resolved, or a newer page replaced it).
pub async fn ack_tag(db: &PgPool, user: &str, tag: &str) -> Result<u64, sqlx::Error> {
    sqlx::query("UPDATE notification_repeat SET acked_at = now() WHERE user_id = $1 AND tag = $2 AND acked_at IS NULL")
        .bind(user)
        .bind(tag)
        .execute(db)
        .await
        .map(|r| r.rows_affected())
}

fn static_kind(kind: &str) -> &'static str {
    match kind {
        "message" => "message",
        "channel_message" => "channel_message",
        "call" => "call",
        "missed_call" => "missed_call",
        _ => "event",
    }
}

/// Send every due, unopened page again. Each due row is claimed (count bumped, next time moved) in one
/// statement, so restarts and replicas never double a repeat. Returns how many were sent.
pub async fn repeat_once(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid) -> Result<usize, sqlx::Error> {
    type Row = (String, String, String, String, String, String, Value);
    let due: Vec<Row> = sqlx::query_as(
        "UPDATE notification_repeat SET sent_count = sent_count + 1, next_at = now() + make_interval(mins => $1) \
         WHERE id IN (SELECT id FROM notification_repeat WHERE acked_at IS NULL AND sent_count < $2 AND next_at <= now() \
                      ORDER BY next_at LIMIT 200 FOR UPDATE SKIP LOCKED) \
         RETURNING user_id, tag, kind, title, body, url, data",
    )
    .bind(REPEAT_EVERY_MINUTES)
    .bind(MAX_REPEATS)
    .fetch_all(db)
    .await?;
    let mut sent = 0;
    for (user, tag, kind, title, body, url, data) in due {
        let msg = PushMessage { kind: static_kind(&kind), title, body, url, tag, data, ttl_secs: 3600, high_urgency: true };
        if web_push::send_to_user(db, transport, vapid, &user, &msg).await > 0 {
            sent += 1;
        }
    }
    Ok(sent)
}

#[derive(serde::Deserialize)]
struct AckQuery {
    t: Option<String>,
}

/// The service worker reports a paged push was opened. The token is the only credential (it was only ever
/// sent to that person's devices); an unknown token is still 204, so nothing can be probed.
async fn ack_route(State(state): State<Arc<ApiState>>, axum::extract::Query(q): axum::extract::Query<AckQuery>) -> Response {
    if let Some(token) = q.t.as_deref().map(str::trim).filter(|t| !t.is_empty() && t.len() <= 64) {
        if let Err(error) = sqlx::query("UPDATE notification_repeat SET acked_at = now() WHERE ack_token = $1 AND acked_at IS NULL").bind(token).execute(&state.db).await {
            return internal(&error.to_string());
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// One held push, as the summary counts it: (event type, tag, severity).
pub type HeldItem = (String, String, Option<String>);

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The summary line for a person's held pushes, or `None` when there is nothing to tell. Messages and
/// missed calls count every push; approvals, threads and problems count each one once however often it
/// was re-sent (same tag). Critical problems come first.
pub fn summary_body(items: &[HeldItem]) -> Option<String> {
    use std::collections::BTreeSet;
    fn critical(i: &HeldItem) -> bool {
        i.0 == "attention.opened" && i.2.as_deref() == Some("critical")
    }
    fn bucket(i: &HeldItem) -> usize {
        match i.0.as_str() {
            _ if critical(i) => 0,
            "message.received" => 1,
            "call.ended" => 2,
            "approval.requested" => 3,
            "thread.needs_user" => 4,
            "attention.opened" => 5,
            // Anything else (usage alerts, sign-in needed, finished runs, ...).
            _ => 6,
        }
    }
    // Messages and missed calls count every push; the rest count each tag once.
    let size = |b: usize| {
        let mine = items.iter().filter(|i| bucket(i) == b);
        if b == 1 || b == 2 {
            mine.count()
        } else {
            mine.map(|i| i.1.as_str()).collect::<BTreeSet<&str>>().len()
        }
    };
    let parts: Vec<String> = [
        (size(0), "urgent problem", "urgent problems"),
        (size(1), "message", "messages"),
        (size(2), "missed call", "missed calls"),
        (size(3), "approval", "approvals"),
        (size(4), "thread", "threads"),
        (size(5), "problem", "problems"),
        (size(6), "update", "updates"),
    ]
    .into_iter()
    .filter(|(n, _, _)| *n > 0)
    .map(|(n, one, many)| plural(n, one, many))
    .collect();
    let single = parts.len() == 1 && parts[0].starts_with("1 ");
    let list = match parts.as_slice() {
        [] => return None,
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    Some(format!("{list} {}", if single { "needs you" } else { "need you" }))
}

pub fn summary_message(body: String) -> PushMessage {
    PushMessage {
        kind: "event",
        title: "While you were away".into(),
        body,
        url: "/".into(),
        tag: "quiet-summary".into(),
        data: json!({ "event": "quiet.summary" }),
        ttl_secs: 24 * 3600,
        high_urgency: false,
    }
}

/// Send the "While you were away" summary to everyone whose quiet hours are over and who has held pushes.
/// Each person's held rows are claimed in one statement, so restarts and replicas never send it twice.
/// Returns how many summaries were sent.
pub async fn summarize_once(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid) -> Result<usize, sqlx::Error> {
    summarize_at(db, transport, vapid, Utc::now()).await
}

async fn summarize_at(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid, now: DateTime<Utc>) -> Result<usize, sqlx::Error> {
    let users: Vec<String> = sqlx::query_scalar("SELECT DISTINCT user_id FROM notification_held WHERE summarized_at IS NULL LIMIT 500").fetch_all(db).await?;
    let mut sent = 0;
    for user in users {
        if quiet_hours(db, &user).await?.active_at(now) {
            continue;
        }
        let items: Vec<HeldItem> = sqlx::query_as("UPDATE notification_held SET summarized_at = now() WHERE user_id = $1 AND summarized_at IS NULL RETURNING event_type, tag, severity").bind(&user).fetch_all(db).await?;
        let Some(body) = summary_body(&items) else { continue };
        if web_push::send_to_user(db, transport, vapid, &user, &summary_message(body)).await > 0 {
            sent += 1;
        }
    }
    Ok(sent)
}

async fn get_quiet_route(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match quiet_hours(&state.db, &user).await {
        Ok(q) => Json(q.json()).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

async fn put_quiet_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Option<Json<Value>>) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let bad = || (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_value" }))).into_response();
    let Some(changes) = body.as_ref().and_then(|b| b.0.as_object()) else { return bad() };
    let mut q = match quiet_hours(&state.db, &user).await {
        Ok(q) => q,
        Err(e) => return internal(&e.to_string()),
    };
    if q.apply(changes).is_err() {
        return bad();
    }
    match set_quiet_hours(&state.db, &user, &q).await {
        Ok(()) => Json(q.json()).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Copy and links
// ---------------------------------------------------------------------------

/// First non-empty string among `keys` (the runtime forwards camelCase, cloud emitters snake_case).
fn s<'a>(data: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| data.get(*k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()))
}

fn num(data: &Value, key: &str) -> Option<f64> {
    data.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

fn clip(text: &str, n: usize) -> String {
    let t: String = text.chars().take(n).collect();
    if text.chars().count() > n {
        format!("{}…", t.trim_end())
    } else {
        t
    }
}

fn enc(v: &str) -> String {
    urlencoding::encode(v).into_owned()
}

/// The push-links scheme the app reads (`allternit-ai` `lib/gateway/push-links.ts`).
/// The answer pages agents make (answer-me-with-html) are served here; a push may open one.
pub const ARTIFACTS_ORIGIN: &str = "https://artifacts.allternit.com/";

fn thread_link(bot: Option<&str>, thread: Option<&str>) -> Option<String> {
    Some(format!("/?allternit_thread={}&bot={}", enc(thread?), enc(bot?)))
}

fn channel_link(provider: Option<&str>) -> Option<String> {
    Some(format!("/?allternit_channel={}", enc(provider?)))
}

/// Name the app shows for an AI subscription provider.
fn provider_name(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "chatgpt" | "openai" | "codex" => "ChatGPT".into(),
        "claude" | "anthropic" => "Claude".into(),
        "gemini" | "google" => "Gemini".into(),
        "grok" | "xai" => "Grok".into(),
        "kimi" | "moonshot" => "Kimi".into(),
        "" => "your AI subscription".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn money(v: f64) -> String {
    format!("${v:.2}")
}

/// The notification for one backbone event, or `None` when this event type
/// (or this particular event) is never pushed.
pub fn message_for(event: &str, data: &Value) -> Option<PushMessage> {
    let bot = s(data, &["bot_id", "botId"]);
    let thread = s(data, &["thread_id", "threadId"]);
    let bot_name = s(data, &["botName", "bot_name"]);
    let thread_title = s(data, &["threadTitle", "thread_title"]);
    let link = thread_link(bot, thread).unwrap_or_else(|| "/".into());
    let who = bot_name.unwrap_or("Your bot");
    let mut msg = PushMessage {
        kind: "event",
        title: String::new(),
        body: String::new(),
        url: link.clone(),
        tag: String::new(),
        data: json!({ "event": event, "botId": bot, "threadId": thread }),
        ttl_secs: 24 * 3600,
        high_urgency: false,
    };
    match event {
        "approval.requested" => {
            if s(data, &["source"]) == Some("gizzi-permission") {
                return None;
            }
            let id = s(data, &["approval_id", "approvalId"]).unwrap_or("approval");
            msg.title = "Approval needed".into();
            msg.body = match s(data, &["summary", "action"]) {
                Some(what) => format!("{who} wants to: {}", clip(what, 200)),
                None => format!("{who} is waiting for your OK."),
            };
            msg.tag = format!("approval-{id}");
            msg.high_urgency = true;
        }
        "approval.resolved" => {
            let id = s(data, &["approval_id", "approvalId"]).unwrap_or("approval");
            let decision = s(data, &["decision"]).unwrap_or("decided");
            msg.title = format!("Approval {decision}");
            msg.body = match s(data, &["summary"]) {
                Some(what) => clip(what, 200),
                None => format!("{who} can carry on."),
            };
            // Replaces the "Approval needed" notification.
            msg.tag = format!("approval-{id}");
        }
        "agent.run.completed" => {
            let id = s(data, &["run_id", "runId"]).unwrap_or("run");
            msg.title = format!("{who} finished a run");
            let what = s(data, &["title"]).or(thread_title);
            msg.body = match (s(data, &["status"]), what) {
                (Some("failed"), _) => "It failed. Open it to see why.".into(),
                (Some("cancelled"), _) => "It was cancelled.".into(),
                (_, Some(t)) => format!("Done: {}", clip(t, 160)),
                _ => "It's done.".into(),
            };
            msg.tag = format!("run-{id}");
        }
        "thread.needs_user" => {
            msg.title = format!("{who} needs you");
            msg.body = s(data, &["reason", "detail"]).map(|r| clip(r, 200)).or(thread_title.map(|t| format!("In “{}”", clip(t, 120)))).unwrap_or_else(|| "Open the thread to reply.".into());
            msg.tag = format!("needs-{}", thread.or(bot).unwrap_or("you"));
            msg.high_urgency = true;
            // An agent waiting on an answer page (answer-me-with-html steering): the notification opens that page.
            // Only artifacts.allternit.com links; the page key rides in the fragment.
            if let Some(page) = s(data, &["url"]).filter(|u| u.starts_with(ARTIFACTS_ORIGIN) && !u.contains(['\n', '\r', ' '])) {
                msg.url = page.to_string();
                msg.tag = format!("needs-page-{}", clip(s(data, &["page_id", "pageId"]).unwrap_or("page"), 40));
            }
        }
        "message.received" => {
            let provider = s(data, &["channel", "provider"]);
            let label = provider.map(web_push::provider_label).unwrap_or("channel");
            msg.title = match s(data, &["from"]) {
                Some(from) => clip(from, 80),
                None => "New message".into(),
            };
            msg.body = match s(data, &["text"]) {
                Some(text) => clip(text, 240),
                None => format!("You have a new {label} message for your bot"),
            };
            msg.url = thread_link(bot, thread).or_else(|| channel_link(provider)).unwrap_or_else(|| "/".into());
            msg.tag = match (thread, provider) {
                (Some(t), _) => format!("msg-{t}"),
                (None, Some(p)) => format!("channel-{p}"),
                _ => "msg".into(),
            };
            msg.kind = "message";
        }
        "call.ended" => {
            let id = s(data, &["call_id", "callId"]).or(thread).unwrap_or("call");
            let from = s(data, &["from"]);
            let missed = data.get("missed").and_then(Value::as_bool) == Some(true);
            if missed {
                msg.title = match from {
                    Some(f) => format!("Missed call from {}", clip(f, 60)),
                    None => "Missed call".into(),
                };
                msg.body = "Open the thread to follow up.".into();
            } else {
                msg.title = "Call ended".into();
                let secs = num(data, "duration_s").or(num(data, "durationSec")).or(num(data, "durationMs").map(|ms| ms / 1000.0));
                let with = from.map(|f| format!(" with {}", clip(f, 60))).unwrap_or_default();
                msg.body = match secs {
                    Some(s) if s >= 60.0 => format!("{who} finished a call{with} ({} min).", (s / 60.0).round() as i64),
                    Some(s) => format!("{who} finished a call{with} ({} s).", s.round() as i64),
                    None => format!("{who} finished a call{with}."),
                };
            }
            msg.tag = format!("call-{id}");
        }
        "inbox.item.created" => {
            let id = s(data, &["item_id", "itemId"]).unwrap_or("inbox");
            msg.title = s(data, &["title"]).map(|t| clip(t, 100)).unwrap_or_else(|| "New inbox item".into());
            msg.body = s(data, &["summary", "detail"]).map(|t| clip(t, 200)).unwrap_or_else(|| "Open your inbox to see it.".into());
            msg.tag = format!("inbox-{id}");
        }
        "vendor.ticket.created" => {
            let id = s(data, &["ticket_id", "ticketId"]).unwrap_or("ticket");
            msg.title = format!("New ticket for {who}");
            msg.body = "A vendor bot has work waiting.".into();
            msg.tag = format!("ticket-{id}");
        }
        "subscription.login_needed" | "subscription.signed_in" => {
            let provider = provider_name(s(data, &["provider"]).unwrap_or(""));
            let login = s(data, &["loginId", "login_id"]).or(s(data, &["provider"])).unwrap_or("subscription");
            let account = s(data, &["account", "label"]).map(|a| format!(" ({})", clip(a, 60))).unwrap_or_default();
            if event == "subscription.login_needed" {
                msg.title = format!("Sign in to {provider} again");
                msg.body = format!("Your {provider} subscription{account} signed out. Open Allternit and sign in so your bots keep working.");
            } else {
                msg.title = format!("{provider} is signed in again");
                msg.body = format!("Your {provider} subscription{account} is working again.");
            }
            // The two replace each other.
            msg.tag = format!("login-{login}");
            msg.url = "/".into();
        }
        "attention.opened" | "attention.resolved" => {
            let id = s(data, &["attention_id", "attentionId"]).unwrap_or("attention");
            let summary = s(data, &["summary"]).map(|t| clip(t, 200));
            if event == "attention.opened" {
                let critical = s(data, &["severity"]) == Some("critical");
                msg.high_urgency = critical;
                msg.title = if critical { "Needs fixing now".into() } else { "Something needs fixing".into() };
                msg.body = summary.unwrap_or_else(|| "Open Allternit to see what happened.".into());
            } else {
                msg.title = "Fixed".into();
                msg.body = summary.unwrap_or_else(|| "A problem was fixed.".into());
            }
            // Opened, re-sent and resolved all share one tag, so the notification is replaced, not stacked.
            msg.tag = format!("attention-{id}");
            msg.url = s(data, &["action_url", "actionUrl"]).filter(|u| u.starts_with('/')).unwrap_or("/").to_string();
        }
        "usage.threshold" => {
            let meter = s(data, &["meter"]).unwrap_or("usage");
            let period = s(data, &["period"]).unwrap_or("");
            let percent = num(data, "percent").unwrap_or(0.0);
            let (used, limit) = (num(data, "used"), num(data, "limit"));
            let what = if meter == "cloud_spend" { "monthly cloud budget" } else { "plan" };
            msg.title = if percent >= 100.0 { format!("You've used all of your {what}") } else { format!("You've used {}% of your {what}", percent.round() as i64) };
            msg.body = match (used, limit) {
                (Some(u), Some(l)) if meter == "cloud_spend" => format!("{} of {} so far this month.", money(u), money(l)),
                (Some(u), Some(l)) => format!("{u} of {l} used."),
                _ => "Open Usage to see where it went.".into(),
            };
            msg.tag = format!("usage-{meter}-{period}");
            msg.url = "/".into();
        }
        _ => return None,
    }
    Some(msg)
}

// ---------------------------------------------------------------------------
// The sink
// ---------------------------------------------------------------------------

/// Set after a sink pass succeeds, cleared when one fails (for example before migration 060 is
/// applied). Direct channel pushes only hand over to the sink while it is working.
static SINK_HEALTHY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn sink_healthy() -> bool {
    SINK_HEALTHY.load(std::sync::atomic::Ordering::Relaxed)
}

/// True when the push sink is working and `runtime_id` forwards its ledger to the backbone (it has
/// sent at least one event), so the sink will push the messages it receives.
pub async fn runtime_forwards_events(db: &PgPool, runtime_id: &str) -> bool {
    if !sink_healthy() {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM platform_events WHERE source = $1)")
        .bind(format!("runtime:{runtime_id}"))
        .fetch_one(db)
        .await
        .unwrap_or(false)
}

/// What the sink did with one event (`notification_push_log.outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    Off,
    Unreachable,
    Skipped,
    /// Quiet hours: recorded in `notification_held` for the summary.
    Held,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Sent => "sent",
            Outcome::Off => "off",
            Outcome::Unreachable => "unreachable",
            Outcome::Skipped => "skipped",
            Outcome::Held => "held",
        }
    }
}

type EventRow = (String, String, String, Value, Option<DateTime<Utc>>);

async fn decide_and_send(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid, row: &EventRow) -> Outcome {
    let (event_id, user, event, data, occurred_at) = row;
    if occurred_at.is_some_and(|t| Utc::now() - t > chrono::Duration::hours(STALE_AFTER_HOURS)) {
        return Outcome::Skipped;
    }
    if matches!(event.as_str(), "approval.resolved" | "attention.resolved") {
        // Resolved: whatever was paging for it stops, whether or not this event is pushed.
        if let Some(m) = message_for(event, data) {
            let _ = ack_tag(db, user, &m.tag).await;
        }
    }
    if !enabled(db, user, event).await {
        return Outcome::Off;
    }
    let Some(msg) = message_for(event, data) else { return Outcome::Skipped };
    if event == "message.received" {
        // The switch-over from the direct channel push: that push already told them.
        let direct: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM push_log WHERE user_id = $1 AND collapse_key LIKE 'channel-%' AND created_at > now() - make_interval(secs => $2))")
            .bind(user)
            .bind(DIRECT_DEDUPE_SECS as f64)
            .fetch_one(db)
            .await
            .unwrap_or(false);
        if direct {
            return Outcome::Skipped;
        }
    }
    let severity = (event == "attention.opened").then(|| s(data, &["severity"])).flatten();
    let lvl = level(event, data);
    if hold_if_quiet(db, user, event, &msg, lvl, severity, Some(event_id.as_str())).await {
        return Outcome::Held;
    }
    let msg = page_if_urgent(db, user, lvl, msg).await;
    if event == "message.received" && !web_push::allow_message_push(db, user, &msg.tag).await {
        return Outcome::Skipped;
    }
    if web_push::send_to_user(db, transport, vapid, user, &msg).await > 0 {
        Outcome::Sent
    } else {
        Outcome::Unreachable
    }
}

/// One pass: claim every recent, not-yet-seen user event and push it when the
/// owner wants it. Returns how many events were handled.
pub async fn sink_once(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid) -> Result<usize, sqlx::Error> {
    let result = sink_pass(db, transport, vapid).await;
    SINK_HEALTHY.store(result.is_ok(), std::sync::atomic::Ordering::Relaxed);
    result
}

async fn sink_pass(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid) -> Result<usize, sqlx::Error> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT e.id, e.user_id, e.type, e.data, e.occurred_at FROM platform_events e \
         WHERE e.subject = 'user' AND e.user_id IS NOT NULL AND e.created_at > now() - make_interval(mins => $1) \
           AND NOT EXISTS (SELECT 1 FROM notification_push_log l WHERE l.event_id = e.id) \
         ORDER BY e.created_at LIMIT $2",
    )
    .bind(SCAN_WINDOW_MINUTES as i32)
    .bind(BATCH)
    .fetch_all(db)
    .await?;
    let mut handled = 0;
    for row in rows {
        let claimed: Option<String> = sqlx::query_scalar("INSERT INTO notification_push_log (event_id, user_id, event_type) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING RETURNING event_id")
            .bind(&row.0)
            .bind(&row.1)
            .bind(&row.2)
            .fetch_optional(db)
            .await?;
        if claimed.is_none() {
            continue;
        }
        let outcome = decide_and_send(db, transport, vapid, &row).await;
        sqlx::query("UPDATE notification_push_log SET outcome = $2 WHERE event_id = $1").bind(&row.0).bind(outcome.as_str()).execute(db).await?;
        handled += 1;
    }
    Ok(handled)
}

/// Start the sink. Does nothing when push isn't configured.
pub fn spawn_push_sink(db: PgPool) {
    let Some(vapid) = Vapid::from_env() else {
        tracing::info!("notifications: push sink off (VAPID env unset)");
        return;
    };
    tokio::spawn(async move {
        let transport = web_push::HttpTransport::new();
        let mut passes: u64 = 0;
        loop {
            if let Err(error) = sink_once(&db, &transport, &vapid).await {
                tracing::warn!("notifications: push sink pass failed: {error}");
            }
            passes += 1;
            // About once a minute: send "While you were away" to people whose quiet hours just ended.
            if passes % 20 == 0 {
                if let Err(error) = summarize_once(&db, &transport, &vapid).await {
                    tracing::warn!("notifications: quiet-hours summary pass failed: {error}");
                }
                if let Err(error) = repeat_once(&db, &transport, &vapid).await {
                    tracing::warn!("notifications: page repeat pass failed: {error}");
                }
            }
            if passes % 1200 == 0 {
                let _ = sqlx::query("DELETE FROM notification_push_log WHERE created_at < now() - interval '2 days'").execute(&db).await;
                let _ = sqlx::query("DELETE FROM notification_held WHERE held_at < now() - interval '7 days'").execute(&db).await;
                let _ = sqlx::query("DELETE FROM notification_repeat WHERE created_at < now() - interval '2 days'").execute(&db).await;
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::allternit_events;
    use crate::routes::test_support::{events_backbone_schema, test_pool};
    use crate::routes::web_push::tests::{browser_sub, decrypt, fixture_keys, Recorder};

    pub(crate) async fn schema(db: &PgPool) {
        events_backbone_schema(db).await;
        for sql in [include_str!("../../migrations_pg/039_push_subscriptions.sql"), include_str!("../../migrations_pg/060_notification_preferences.sql"), include_str!("../../migrations_pg/102_notification_quiet_hours.sql"), include_str!("../../migrations_pg/107_quiet_hours_levels.sql")] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(db).await.expect("migration applies");
        }
    }

    fn vapid() -> Vapid {
        let (public, private) = fixture_keys();
        Vapid::new(&public, &private, "mailto:test@allternit.com").unwrap()
    }

    /// A device for `user`; returns what decrypting its pushes needs.
    async fn device(db: &PgPool, user: &str) -> (aws_lc_rs::agreement::PrivateKey, Vec<u8>, [u8; 16]) {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let (p256dh, auth_b64, key, auth) = browser_sub();
        web_push::subscribe(db, user, "phone", &format!("https://push.example.com/{user}"), &p256dh, &auth_b64, "test").await.unwrap();
        (key, URL_SAFE_NO_PAD.decode(&p256dh).unwrap(), auth)
    }

    async fn emit(db: &PgPool, user: &str, name: &str, data: Value) -> String {
        allternit_events::emit_user_event(db, user, name, &data, None, "cloud", None).await.unwrap().unwrap()
    }

    async fn outcome(db: &PgPool, event: &str) -> String {
        sqlx::query_scalar("SELECT outcome FROM notification_push_log WHERE event_id = $1").bind(event).fetch_one(db).await.unwrap()
    }

    #[test]
    fn every_pushable_event_has_a_default_and_copy() {
        for e in pushable() {
            assert!(DEFAULTS.iter().any(|(n, _)| *n == e.name), "default for {}", e.name);
            let m = message_for(e.name, &json!({ "bot_id": "b1", "thread_id": "t1" })).unwrap_or_else(|| panic!("copy for {}", e.name));
            assert!(!m.title.is_empty() && !m.body.is_empty() && !m.tag.is_empty(), "{}: {m:?}", e.name);
            assert!(m.url.starts_with('/'), "{}", e.name);
            assert!(!m.title.contains('.') || m.title.contains("…"), "plain title for {}: {}", e.name, m.title);
        }
        // Platform-only events are never a person's notification.
        assert!(message_for("message.status", &json!({})).is_none());
        assert!(!pushable().any(|e| e.name == "registration.updated"));
        // Defaults keep what the app did: messages and calls on; finished runs off.
        for (name, on) in [("approval.requested", true), ("thread.needs_user", true), ("call.ended", true), ("subscription.login_needed", true), ("message.received", true), ("agent.run.completed", false)] {
            assert_eq!(default_enabled(name), on, "{name}");
        }
    }

    #[test]
    fn copy_and_links_per_type() {
        let m = message_for("approval.requested", &json!({ "bot_id": "b 1", "thread_id": "t/1", "approval_id": "ap1", "summary": "send the invoice email", "botName": "Olive" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str()), ("Approval needed", "Olive wants to: send the invoice email"));
        assert_eq!(m.url, "/?allternit_thread=t%2F1&bot=b%201");
        assert_eq!(m.tag, "approval-ap1");
        assert!(m.high_urgency);
        let r = message_for("approval.resolved", &json!({ "approvalId": "ap1", "decision": "approved" })).unwrap();
        assert_eq!((r.title.as_str(), r.tag.as_str()), ("Approval approved", "approval-ap1"), "replaces the request");
        // Coding-session permission asks go through the remote-control lane instead.
        assert!(message_for("approval.requested", &json!({ "approvalId": "x", "source": "gizzi-permission" })).is_none());

        let m = message_for("message.received", &json!({ "channel": "telegram", "from": "Sam", "text": "are you there?" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str(), m.url.as_str(), m.tag.as_str()), ("Sam", "are you there?", "/?allternit_channel=telegram", "channel-telegram"));
        assert_eq!(m.kind, "message");
        let m = message_for("message.received", &json!({ "provider": "slack", "bot_id": "b1", "thread_id": "t1" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str(), m.url.as_str(), m.tag.as_str()), ("New message", "You have a new Slack message for your bot", "/?allternit_thread=t1&bot=b1", "msg-t1"));

        let m = message_for("thread.needs_user", &json!({ "botName": "Olive", "thread_id": "t1", "bot_id": "b1", "reason": "Which date works?" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str(), m.tag.as_str()), ("Olive needs you", "Which date works?", "needs-t1"));
        // An answer page link opens the page itself; any other URL is ignored.
        let m = message_for("thread.needs_user", &json!({ "botName": "Claude Code", "reason": "Which database?", "url": "https://artifacts.allternit.com/p-abc.html#k=1f", "page_id": "abc" })).unwrap();
        assert_eq!((m.url.as_str(), m.tag.as_str()), ("https://artifacts.allternit.com/p-abc.html#k=1f", "needs-page-abc"));
        let m = message_for("thread.needs_user", &json!({ "botName": "X", "url": "https://evil.example/a" })).unwrap();
        assert_eq!(m.url.as_str(), "/");

        let m = message_for("call.ended", &json!({ "missed": true, "from": "+1 555 0100", "call_id": "c1" })).unwrap();
        assert_eq!((m.title.as_str(), m.tag.as_str()), ("Missed call from +1 555 0100", "call-c1"));
        let m = message_for("call.ended", &json!({ "missed": false, "duration_s": 125, "botName": "Olive" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str()), ("Call ended", "Olive finished a call (2 min)."));

        let m = message_for("subscription.login_needed", &json!({ "provider": "chatgpt", "loginId": "l1", "label": "work" })).unwrap();
        assert_eq!(m.title, "Sign in to ChatGPT again");
        assert_eq!(m.body, "Your ChatGPT subscription (work) signed out. Open Allternit and sign in so your bots keep working.");
        assert_eq!((m.tag.as_str(), m.url.as_str()), ("login-l1", "/"));
        let back = message_for("subscription.signed_in", &json!({ "provider": "chatgpt", "loginId": "l1" })).unwrap();
        assert_eq!(back.tag, m.tag, "signed in replaces sign-in needed");

        let m = message_for("attention.opened", &json!({ "attentionId": "routine_failed:r1", "severity": "warn", "summary": "Morning briefing failed: the bot is signed out.", "actionUrl": "/?bot=b1" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str(), m.tag.as_str(), m.url.as_str()), ("Something needs fixing", "Morning briefing failed: the bot is signed out.", "attention-routine_failed:r1", "/?bot=b1"));
        let m = message_for("attention.opened", &json!({ "attention_id": "x", "severity": "critical", "summary": "s", "action_url": "https://evil.example" })).unwrap();
        assert_eq!((m.title.as_str(), m.url.as_str()), ("Needs fixing now", "/"));
        assert!(default_enabled("attention.opened") && !default_enabled("attention.resolved"));

        let m = message_for("usage.threshold", &json!({ "meter": "cloud_spend", "percent": 80.0, "used": 81.5, "limit": 100.0, "period": "2026-10" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str(), m.tag.as_str()), ("You've used 80% of your monthly cloud budget", "$81.50 of $100.00 so far this month.", "usage-cloud_spend-2026-10"));
        let m = message_for("usage.threshold", &json!({ "meter": "cloud_spend", "percent": 100.0, "used": 120.0, "limit": 100.0, "period": "2026-10" })).unwrap();
        assert_eq!(m.title, "You've used all of your monthly cloud budget");

        let m = message_for("agent.run.completed", &json!({ "status": "failed", "runId": "r1" })).unwrap();
        assert_eq!((m.title.as_str(), m.body.as_str(), m.tag.as_str()), ("Your bot finished a run", "It failed. Open it to see why.", "run-r1"));
        // Long text is cut.
        let m = message_for("message.received", &json!({ "text": "x".repeat(1000) })).unwrap();
        assert!(m.body.chars().count() <= 241);
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn quiet_window_wraps_midnight_and_follows_the_zone() {
        // Same-day window [09:00, 17:00).
        assert!(in_window(540, 1020, 540) && in_window(540, 1020, 1019));
        assert!(!in_window(540, 1020, 1020) && !in_window(540, 1020, 539));
        // Wrapping window [22:00, 07:00).
        for m in [1320, 1439, 0, 419] {
            assert!(in_window(1320, 420, m), "{m}");
        }
        for m in [420, 600, 1319] {
            assert!(!in_window(1320, 420, m), "{m}");
        }
        // start == end: no quiet hours at all.
        assert!(!(0..1440).any(|m| in_window(600, 600, m)));

        assert_eq!((parse_hhmm("00:00"), parse_hhmm("07:05"), parse_hhmm("23:59")), (Some(0), Some(425), Some(1439)));
        for bad in ["24:00", "7:00", "07:60", "0700", "ab:cd", "", "07:00:00", "-1:00"] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }

        // 22:00–07:00 New York: the same UTC instant is in or out depending on daylight saving time.
        let ny = QuietHours { enabled: true, time_zone: "America/New_York".into(), ..QuietHours::default() };
        assert!(ny.active_at(utc("2026-01-15T03:30:00Z")), "22:30 EST");
        assert!(!ny.active_at(utc("2026-01-15T02:30:00Z")), "21:30 EST");
        assert!(ny.active_at(utc("2026-07-15T02:30:00Z")), "22:30 EDT");
        assert!(ny.active_at(utc("2026-07-15T10:59:00Z")), "06:59 EDT");
        assert!(!ny.active_at(utc("2026-07-15T11:00:00Z")), "07:00 EDT ends it");
        assert!(ny.active_at(utc("2026-01-15T11:30:00Z")), "06:30 EST");
        // The night the clocks go forward (2026-03-08, 02:00 → 03:00): still quiet through the jump, out at 07:00 EDT.
        assert!(ny.active_at(utc("2026-03-08T07:30:00Z")), "03:30 EDT");
        assert!(!ny.active_at(utc("2026-03-08T11:00:00Z")), "07:00 EDT");
        // Off, or start == end: never quiet.
        assert!(!QuietHours { enabled: false, ..ny.clone() }.active_at(utc("2026-01-15T03:30:00Z")));
        assert!(!QuietHours { start_min: 600, end_min: 600, ..ny.clone() }.active_at(utc("2026-01-15T15:00:00Z")));
        // Default: off, 22:00–07:00 UTC, calls ring, urgent alerts get through.
        assert_eq!(QuietHours::default().json(), json!({ "enabled": false, "start": "22:00", "end": "07:00", "timeZone": "UTC", "allowCalls": true, "letThrough": "urgent" }));
    }

    #[test]
    fn quiet_hours_put_validation() {
        let mut q = QuietHours::default();
        q.apply(json!({ "enabled": true, "start": "23:15", "timeZone": "Europe/London" }).as_object().unwrap()).unwrap();
        assert_eq!(q.json(), json!({ "enabled": true, "start": "23:15", "end": "07:00", "timeZone": "Europe/London", "allowCalls": true, "letThrough": "urgent" }));
        q.apply(json!({ "allowCalls": false }).as_object().unwrap()).unwrap();
        q.apply(json!({ "letThrough": "needs_you" }).as_object().unwrap()).unwrap();
        assert_eq!(q.let_through, Some(Level::NeedsYou));
        q.apply(json!({ "letThrough": "none" }).as_object().unwrap()).unwrap();
        assert_eq!((q.let_through, q.json()["letThrough"].as_str()), (None, Some("none")));
        assert!(!q.allow_calls && q.enabled, "a subset leaves the rest alone");
        for bad in [json!({}), json!({ "start": "25:00" }), json!({ "timeZone": "Mars/Base" }), json!({ "enabled": "yes" }), json!({ "end": 420 }), json!({ "quiet": true }), json!({ "letThrough": "all" }), json!({ "letThrough": true })] {
            let before = q.clone();
            assert_eq!(q.clone().apply(bad.as_object().unwrap()), Err("bad_value"), "{bad}");
            assert_eq!(q, before);
        }
    }

    #[test]
    fn levels_and_what_quiet_hours_let_through() {
        assert_eq!(level("attention.opened", &json!({ "severity": "critical" })), Level::Urgent);
        assert_eq!(level("attention.opened", &json!({ "severity": "warn" })), Level::Normal);
        assert_eq!(level("attention.opened", &json!({ "severity": "info" })), Level::Low);
        assert_eq!(level("approval.requested", &json!({})), Level::NeedsYou);
        assert_eq!(level("thread.needs_user", &json!({})), Level::NeedsYou);
        assert_eq!(level("message.received", &json!({})), Level::Normal);
        assert_eq!(level("agent.run.completed", &json!({})), Level::Low);
        // A bot can mark its own message urgent.
        assert_eq!(level("message.received", &json!({ "urgency": "urgent" })), Level::Urgent);
        assert_eq!(level("thread.needs_user", &json!({ "urgency": "normal" })), Level::NeedsYou);

        let now = Utc::now();
        let mut q = QuietHours { enabled: true, start_min: 0, end_min: 1439, ..QuietHours::default() };
        if !q.active_at(now) {
            return;
        }
        // Default: only urgent gets through.
        assert!(!q.holds_at(Level::Urgent, now));
        assert!(q.holds_at(Level::NeedsYou, now) && q.holds_at(Level::Normal, now));
        q.let_through = Some(Level::NeedsYou);
        assert!(!q.holds_at(Level::NeedsYou, now) && q.holds_at(Level::Normal, now));
        q.let_through = None;
        assert!(q.holds_at(Level::Urgent, now));
        // Not in quiet hours: nothing is held.
        q.enabled = false;
        assert!(!q.holds_at(Level::Low, now));
    }

    #[tokio::test]
    async fn an_urgent_push_in_quiet_hours_pages_until_opened() {
        let db = test_pool().await;
        schema(&db).await;
        let vapid = vapid();
        let (key, public, auth) = device(&db, "u1").await;
        let rec = Recorder::default();
        let q = QuietHours { enabled: true, start_min: 0, end_min: 1439, ..QuietHours::default() };
        set_quiet_hours(&db, "u1", &q).await.unwrap();
        if !q.active_at(Utc::now()) {
            return;
        }
        let normal = emit(&db, "u1", "message.received", json!({ "thread_id": "t1", "bot_id": "b1", "text": "hi" })).await;
        let urgent = emit(&db, "u1", "attention.opened", json!({ "attention_id": "a1", "severity": "critical", "summary": "Site is down." })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &normal).await, "held");
        assert_eq!(outcome(&db, &urgent).await, "sent", "urgent gets through by default");
        let payload: Value = serde_json::from_slice(&decrypt(&rec.sent.lock().unwrap()[0].2, &key, &public, &auth)).unwrap();
        let ack = payload["data"]["ack"].as_str().expect("paged push carries its ack link").to_string();
        assert!(ack.contains("/api/v1/notifications/ack?t="), "{ack}");

        // Not due yet: nothing repeats.
        assert_eq!(repeat_once(&db, &rec, &vapid).await.unwrap(), 0);
        let due = || sqlx::query("UPDATE notification_repeat SET next_at = now() - interval '1 second' WHERE acked_at IS NULL").execute(&db);
        for _ in 0..MAX_REPEATS {
            due().await.unwrap();
            assert_eq!(repeat_once(&db, &rec, &vapid).await.unwrap(), 1);
        }
        due().await.unwrap();
        assert_eq!(repeat_once(&db, &rec, &vapid).await.unwrap(), 0, "at most MAX_REPEATS resends");
        assert_eq!(rec.sent.lock().unwrap().len(), 1 + MAX_REPEATS as usize);

        // A second page, opened on the phone: no more repeats.
        sqlx::query("DELETE FROM notification_repeat").execute(&db).await.unwrap();
        let again = emit(&db, "u1", "attention.opened", json!({ "attention_id": "a2", "severity": "critical", "summary": "Still down." })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &again).await, "sent");
        let token: String = sqlx::query_scalar("SELECT ack_token FROM notification_repeat WHERE tag = 'attention-a2'").fetch_one(&db).await.unwrap();
        sqlx::query("UPDATE notification_repeat SET acked_at = now() WHERE ack_token = $1").bind(&token).execute(&db).await.unwrap();
        due().await.unwrap();
        assert_eq!(repeat_once(&db, &rec, &vapid).await.unwrap(), 0, "opened: stops");

        // A third page, resolved: the resolve event stops it.
        let third = emit(&db, "u1", "attention.opened", json!({ "attention_id": "a3", "severity": "critical", "summary": "Down again." })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &third).await, "sent");
        emit(&db, "u1", "attention.resolved", json!({ "attention_id": "a3", "summary": "Back up." })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        due().await.unwrap();
        assert_eq!(repeat_once(&db, &rec, &vapid).await.unwrap(), 0, "resolved: stops");
    }

    #[test]
    fn summary_copy_counts_by_kind() {
        let item = |e: &str, tag: &str, sev: Option<&str>| (e.to_string(), tag.to_string(), sev.map(str::to_string));
        let items = vec![
            item("message.received", "msg-t1", None),
            item("message.received", "msg-t1", None),
            item("message.received", "dm-2", None),
            item("approval.requested", "approval-a1", None),
            item("thread.needs_user", "needs-t1", None),
            item("thread.needs_user", "needs-t2", None),
            item("thread.needs_user", "needs-t2", None),
        ];
        assert_eq!(summary_body(&items).unwrap(), "3 messages, 1 approval and 2 threads need you");
        // Critical problems first; a re-sent problem counts once.
        let mut more = items.clone();
        more.push(item("attention.opened", "attention-x", Some("critical")));
        more.push(item("attention.opened", "attention-x", Some("critical")));
        more.push(item("attention.opened", "attention-y", Some("warn")));
        more.push(item("call.ended", "call-r1", None));
        more.push(item("usage.threshold", "usage-plan-2026-10", None));
        assert_eq!(summary_body(&more).unwrap(), "1 urgent problem, 3 messages, 1 missed call, 1 approval, 2 threads, 1 problem and 1 update need you");
        assert_eq!(summary_body(&[item("message.received", "dm-1", None)]).unwrap(), "1 message needs you");
        assert_eq!(summary_body(&[item("call.ended", "call-1", None), item("call.ended", "call-2", None)]).unwrap(), "2 missed calls need you");
        assert_eq!(summary_body(&[]), None);
        let m = summary_message("1 message needs you".into());
        assert_eq!((m.title.as_str(), m.url.as_str(), m.tag.as_str()), ("While you were away", "/", "quiet-summary"));
    }

    #[tokio::test]
    async fn quiet_hours_hold_then_one_summary() {
        let db = test_pool().await;
        schema(&db).await;
        let vapid = vapid();
        let (key, public, auth) = device(&db, "u1").await;
        let rec = Recorder::default();
        // Quiet all day except 23:59 UTC: a window that is on now in any test run but one minute.
        let mut q = QuietHours { enabled: true, start_min: 0, end_min: 1439, ..QuietHours::default() };
        set_quiet_hours(&db, "u1", &q).await.unwrap();
        if !q.active_at(Utc::now()) {
            return;
        }
        let a = emit(&db, "u1", "approval.requested", json!({ "approval_id": "ap1" })).await;
        let m1 = emit(&db, "u1", "message.received", json!({ "thread_id": "t1", "bot_id": "b1", "text": "hi" })).await;
        let m2 = emit(&db, "u1", "message.received", json!({ "thread_id": "t1", "bot_id": "b1", "text": "again" })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        for e in [&a, &m1, &m2] {
            assert_eq!(outcome(&db, e).await, "held");
        }
        assert_eq!(rec.sent.lock().unwrap().len(), 0, "nothing sent in quiet hours");
        // Still quiet: no summary.
        assert_eq!(summarize_once(&db, &rec, &vapid).await.unwrap(), 0);
        // Quiet hours over: one summary, then nothing more.
        q.enabled = false;
        set_quiet_hours(&db, "u1", &q).await.unwrap();
        assert_eq!(summarize_once(&db, &rec, &vapid).await.unwrap(), 1);
        assert_eq!(summarize_once(&db, &rec, &vapid).await.unwrap(), 0, "claimed once");
        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let payload: Value = serde_json::from_slice(&decrypt(&sent[0].2, &key, &public, &auth)).unwrap();
        assert_eq!((payload["title"].as_str(), payload["body"].as_str(), payload["tag"].as_str(), payload["url"].as_str()), (Some("While you were away"), Some("2 messages and 1 approval need you"), Some("quiet-summary"), Some("/")));
    }

    #[tokio::test]
    async fn preferences_default_change_and_reset() {
        let db = test_pool().await;
        schema(&db).await;
        let v = preferences(&db, "u1").await.unwrap();
        let list = v["preferences"].as_array().unwrap();
        assert_eq!(list.len(), pushable().count());
        let row = |v: &Value, n: &str| v["preferences"].as_array().unwrap().iter().find(|p| p["event"] == n).unwrap().clone();
        assert_eq!(row(&v, "approval.requested")["title"], "Approval requests");
        assert_eq!((row(&v, "approval.requested")["enabled"].clone(), row(&v, "agent.run.completed")["enabled"].clone()), (json!(true), json!(false)));
        let changes = json!({ "approval.requested": false, "agent.run.completed": true });
        set_preferences(&db, "u1", changes.as_object().unwrap()).await.unwrap();
        let v = preferences(&db, "u1").await.unwrap();
        assert_eq!(row(&v, "approval.requested")["enabled"], false);
        assert_eq!(row(&v, "approval.requested")["default"], true);
        assert_eq!(row(&v, "agent.run.completed")["enabled"], true);
        assert!(enabled(&db, "u2", "approval.requested").await, "per user");
        set_preferences(&db, "u1", json!({ "approval.requested": null }).as_object().unwrap()).await.unwrap();
        assert!(enabled(&db, "u1", "approval.requested").await, "null = back to default");
        // All or nothing, registry names only.
        assert_eq!(set_preferences(&db, "u1", json!({ "agent.run.completed": false, "nope": true }).as_object().unwrap()).await, Err(PrefError::Bad("unknown_event")));
        assert!(enabled(&db, "u1", "agent.run.completed").await, "the bad batch changed nothing");
        assert_eq!(set_preferences(&db, "u1", json!({ "message.status": true }).as_object().unwrap()).await, Err(PrefError::Bad("unknown_event")));
        assert_eq!(set_preferences(&db, "u1", json!({ "call.ended": "yes" }).as_object().unwrap()).await, Err(PrefError::Bad("bad_value")));
        assert_eq!(set_preferences(&db, "u1", &Map::new()).await, Err(PrefError::Bad("bad_value")));
    }

    #[tokio::test]
    async fn the_sink_respects_preferences_and_pushes_each_event_once() {
        let db = test_pool().await;
        schema(&db).await;
        let vapid = vapid();
        let (key, public, auth) = device(&db, "u1").await;
        device(&db, "u2").await;
        let rec = Recorder::default();

        let wanted = emit(&db, "u1", "approval.requested", json!({ "bot_id": "b1", "thread_id": "t1", "approval_id": "ap1", "summary": "pay $20" })).await;
        let quiet = emit(&db, "u1", "agent.run.completed", json!({ "bot_id": "b1", "run_id": "r1" })).await;
        let theirs_off = emit(&db, "u2", "thread.needs_user", json!({})).await;
        set_preferences(&db, "u2", json!({ "thread.needs_user": false }).as_object().unwrap()).await.unwrap();
        let nobody = emit(&db, "u3", "approval.requested", json!({})).await;

        assert_eq!(sink_once(&db, &rec, &vapid).await.unwrap(), 4);
        assert_eq!(outcome(&db, &wanted).await, "sent");
        assert_eq!(outcome(&db, &quiet).await, "off", "finished runs are off by default");
        assert_eq!(outcome(&db, &theirs_off).await, "off", "u2 turned it off");
        assert_eq!(outcome(&db, &nobody).await, "unreachable", "no device registered");
        {
            let sent = rec.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].0, "https://push.example.com/u1");
            let payload: Value = serde_json::from_slice(&decrypt(&sent[0].2, &key, &public, &auth)).unwrap();
            assert_eq!(payload["title"], "Approval needed");
            assert_eq!(payload["body"], "Your bot wants to: pay $20");
            assert_eq!(payload["url"], "/?allternit_thread=t1&bot=b1");
            assert_eq!(payload["data"]["event"], "approval.requested");
        }
        // A second pass (or a second replica) sends nothing again.
        assert_eq!(sink_once(&db, &rec, &vapid).await.unwrap(), 0);
        assert_eq!(rec.sent.lock().unwrap().len(), 1);

        // Turning a type on makes the next event of it push.
        set_preferences(&db, "u1", json!({ "agent.run.completed": true }).as_object().unwrap()).await.unwrap();
        let now_on = emit(&db, "u1", "agent.run.completed", json!({ "run_id": "r2" })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &now_on).await, "sent");

        // Stale at the producer: not pushed.
        let old = allternit_events::emit_user_event(&db, "u1", "thread.needs_user", &json!({}), Some(Utc::now() - chrono::Duration::hours(STALE_AFTER_HOURS + 1)), "cloud", None).await.unwrap().unwrap();
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &old).await, "skipped");
        // Events stored before the scan window are left alone.
        let ancient = emit(&db, "u1", "thread.needs_user", json!({})).await;
        sqlx::query("UPDATE platform_events SET created_at = now() - interval '1 hour' WHERE id = $1").bind(&ancient).execute(&db).await.unwrap();
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notification_push_log WHERE event_id = $1").bind(&ancient).fetch_one(&db).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_relayed_channel_message_is_pushed_once_across_the_switch_over() {
        let db = test_pool().await;
        schema(&db).await;
        let vapid = vapid();
        device(&db, "u1").await;
        let rec = Recorder::default();

        sink_once(&db, &rec, &vapid).await.unwrap();
        assert!(sink_healthy());
        // Old runtime (never forwarded): the direct channel push goes out.
        assert!(!runtime_forwards_events(&db, "rt-1").await);
        assert!(web_push::channel_push_allowed(&db, "u1", "route-1", "rt-1").await);
        // The same message then arrives forwarded from the runtime (its first forwarded event).
        let fwd = allternit_events::emit_user_event(&db, "u1", "message.received", &json!({ "channel": "telegram", "text": "hi" }), None, "runtime:rt-1", Some("rt:be:1")).await.unwrap().unwrap();
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &fwd).await, "skipped", "the direct push already told them");
        assert_eq!(rec.sent.lock().unwrap().len(), 0);

        // From now on that runtime forwards: the direct push is skipped and the sink sends it.
        assert!(runtime_forwards_events(&db, "rt-1").await);
        sqlx::query("UPDATE push_log SET created_at = now() - interval '1 hour'").execute(&db).await.unwrap();
        assert!(!web_push::channel_push_allowed(&db, "u1", "route-1", "rt-1").await, "no direct push for a forwarding runtime");
        let next = allternit_events::emit_user_event(&db, "u1", "message.received", &json!({ "channel": "telegram", "text": "hello again" }), None, "runtime:rt-1", Some("rt:be:2")).await.unwrap().unwrap();
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &next).await, "sent");
        assert_eq!(rec.sent.lock().unwrap().len(), 1, "exactly one push for the message");

        // The message preference covers both paths.
        set_preferences(&db, "u1", json!({ "message.received": false }).as_object().unwrap()).await.unwrap();
        sqlx::query("UPDATE push_log SET created_at = now() - interval '1 hour'").execute(&db).await.unwrap();
        assert!(!web_push::channel_push_allowed(&db, "u1", "route-2", "rt-old").await, "direct push off too");
        let off = allternit_events::emit_user_event(&db, "u1", "message.received", &json!({ "channel": "telegram" }), None, "runtime:rt-1", Some("rt:be:3")).await.unwrap().unwrap();
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &off).await, "off");
    }

    /// The full router builds (no axum route-overlap panic) with these routes in it.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_full_router_builds() {
        let state = crate::routes::test_support::test_state(Arc::new(crate::routes::test_support::MockGateway::new(None, vec![]))).await;
        let _ = crate::create_router(state);
    }
}
