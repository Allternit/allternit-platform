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
//! `source = "gizzi-permission"`) reach browsers through the remote-control lane
//! (`services/remote-control-push`), so the sink sends them to iPhones (APNs) only,
//! never as a second Web Push.
//!
//! ## What every sink push carries (`data`)
//!
//! `event`, `eventId`, `botId`, `threadId`, plus when the event has them `inboxId`, `runtimeId`,
//! `sessionId`, `contentHash`, `senderKind`, `botAvatar` (https only); always `private` (bool),
//! `level` (`low | normal | needs_you | urgent`) and, when it has buttons, `category` ([`category`]).
//! A push for an inbox item uses the item as its tag (`approval:ap1` → `approval-ap1`), and an
//! approval opens `/?allternit_inbox=<id>`. [`super::apns::payload`] turns these into the per-device
//! APNs alert; Web Push sends them as they are.
//!
//! Finished runs (`agent.run.completed`) push by default only when the run failed or took
//! [`LONG_RUN_SECS`] or longer ([`run_worth_pushing`]); turned on explicitly, every run pushes;
//! turned off, none does.
//!
//! ## Status snapshots and revoke
//!
//! The runtime forwards `status.snapshot` (never a notification). [`ingest_snapshot`] keeps the
//! latest per runtime in `status_snapshots`; every inbox id that left `needsYou` since the last one
//! has its delivered notifications (`notification_delivered`) removed from the person's iPhones by a
//! background push ([`super::apns::revoke`]).
//!
//! ## Focus filter
//!
//! - `PUT /api/v1/notifications/focus` `{deviceId, active, allowedBotIds: [..] | null, allowApprovals?}` →
//!   `{ok, deviceId, focus}`. 400 `bad_value`, 404 `unknown_device`. While `active` and confirmed in the
//!   last 24 hours, alerts to that iPhone from bots not in `allowedBotIds` (`null`: no bot) arrive as
//!   `passive` with no sound; nothing is dropped. Approvals keep their level when `allowApprovals`
//!   (default true).
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
    // Only failed or long runs unless turned on explicitly (see `run_worth_pushing`).
    ("agent.run.completed", true),
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
        .route("/api/v1/notifications/focus", axum::routing::put(put_focus_route))
}

/// Event types a person can get pushed: every user-subject event in the registry.
pub fn pushable() -> impl Iterator<Item = &'static EventType> {
    REGISTRY.iter().filter(|e| e.agents || e.bot)
}

pub fn default_enabled(event: &str) -> bool {
    DEFAULTS.iter().find(|(n, _)| *n == event).is_some_and(|(_, on)| *on)
}

/// The switch `user` set for `event`, or `None` when they never changed it (or it can't be read).
pub async fn preference(db: &PgPool, user: &str, event: &str) -> Option<bool> {
    match sqlx::query_scalar::<_, bool>("SELECT enabled FROM notification_preferences WHERE user_id = $1 AND event_type = $2").bind(user).bind(event).fetch_optional(db).await {
        Ok(on) => on,
        Err(error) => {
            tracing::warn!("notifications: reading preference failed: {error}");
            None
        }
    }
}

/// Whether `user` wants pushes for `event`. A read error falls back to the default.
pub async fn enabled(db: &PgPool, user: &str, event: &str) -> bool {
    preference(db, user, event).await.unwrap_or_else(|| default_enabled(event))
}

/// A successful run this long or longer is worth a push by default.
pub const LONG_RUN_SECS: f64 = 120.0;

/// The default rule for `agent.run.completed`: push a failed run, or one that ran [`LONG_RUN_SECS`] or
/// longer. A short or unmeasured successful run does not push (the Live Activity's end state covers it).
pub fn run_worth_pushing(data: &Value) -> bool {
    if matches!(s(data, &["status"]), Some("failed" | "error" | "errored" | "timeout" | "timed_out")) {
        return true;
    }
    let secs = num(data, "durationMs").or(num(data, "duration_ms")).map(|ms| ms / 1000.0).or(num(data, "durationSec")).or(num(data, "duration_s"));
    secs.is_some_and(|s| s >= LONG_RUN_SECS)
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
    enrich(&mut msg, event, data);
    Some(msg)
}

/// Inbox kinds the owner approves or denies (the runtime's `inboxKind`).
pub const APPROVAL_KINDS: [&str; 5] = ["approval", "held_send", "email_draft", "factory_approval", "gizzi_permission"];

/// The notification buttons for an event (`APPROVAL_INLINE`, `APPROVAL_REVIEW`, `BOT_REPLY`, `RUN_DONE`,
/// `NEEDS_FIX`; the app registers the same identifiers), chosen from `inboxKind` before the event name so an
/// email draft that arrives as `thread.needs_user` still gets approval buttons. Inline Approve only when the
/// runtime says the whole text and recipient fit (`approveInline`) and sent the `contentHash` the approve call
/// must echo. Approval buttons need an `inboxId` to act on.
pub fn category(event: &str, data: &Value) -> Option<&'static str> {
    let inbox = s(data, &["inboxId", "inbox_id"]);
    let kind = s(data, &["inboxKind", "inbox_kind"]);
    if kind.is_some_and(|k| APPROVAL_KINDS.contains(&k)) {
        inbox?;
        let inline = data.get("approveInline").and_then(Value::as_bool) == Some(true) && s(data, &["contentHash", "content_hash"]).is_some();
        return Some(if inline { "APPROVAL_INLINE" } else { "APPROVAL_REVIEW" });
    }
    match event {
        "approval.requested" if inbox.is_some() => Some("APPROVAL_REVIEW"),
        "message.received" if s(data, &["senderKind", "sender_kind"]) == Some("bot") => Some("BOT_REPLY"),
        "thread.needs_user" if kind.is_none() => Some("BOT_REPLY"),
        "agent.run.completed" => Some("RUN_DONE"),
        "attention.opened" => Some("NEEDS_FIX"),
        _ => None,
    }
}

/// The notification tag for an inbox item: every run of characters other than letters, digits, `_` and `-`
/// becomes one `-` (`approval:ap1` → `approval-ap1`), the same rule as the web worker (allternit-ai
/// `public/inbox-action-sw.js` `inboxTag`), so its result notification replaces this one.
pub fn inbox_tag(inbox_id: &str) -> String {
    let mut out = String::new();
    for c in inbox_id.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    clip(&out, 120)
}

fn level_name(l: Level) -> &'static str {
    match l {
        Level::Low => "low",
        Level::Normal => "normal",
        Level::NeedsYou => "needs_you",
        Level::Urgent => "urgent",
    }
}

/// The `data` keys every push of a backbone event carries (see the module docs), and the inbox item's tag
/// and link.
fn enrich(msg: &mut PushMessage, event: &str, data: &Value) {
    let mut inbox = s(data, &["inboxId", "inbox_id"]).map(str::to_string);
    if inbox.is_none() && matches!(event, "attention.opened" | "attention.resolved") {
        inbox = s(data, &["attention_id", "attentionId"]).map(|id| format!("attention:{id}"));
    }
    let category = category(event, data);
    let private = data.get("private").and_then(Value::as_bool).unwrap_or(matches!(event, "approval.requested" | "message.received" | "thread.needs_user"));
    let avatar = s(data, &["botAvatar", "botAvatarUrl", "avatarUrl"]).filter(|u| u.starts_with("https://") && !u.contains(['\n', '\r', ' ']));
    let Some(d) = msg.data.as_object_mut() else { return };
    for (out, keys) in [("runtimeId", ["runtimeId", "runtime_id"]), ("sessionId", ["sessionId", "session_id"]), ("contentHash", ["contentHash", "content_hash"]), ("senderKind", ["senderKind", "sender_kind"])] {
        if let Some(v) = s(data, &keys) {
            d.insert(out.into(), json!(v));
        }
    }
    if let Some(a) = avatar {
        d.insert("botAvatar".into(), json!(a));
    }
    if let Some(c) = category {
        d.insert("category".into(), json!(c));
    }
    d.insert("private".into(), json!(private));
    d.insert("level".into(), json!(level_name(level(event, data))));
    if let Some(id) = inbox {
        d.insert("inboxId".into(), json!(id));
        msg.tag = inbox_tag(&id);
        if category.is_some_and(|c| c.starts_with("APPROVAL_")) {
            msg.url = format!("/?allternit_inbox={}", enc(&id));
        }
    }
}

// ---------------------------------------------------------------------------
// Focus filter
// ---------------------------------------------------------------------------

/// Most bots one Focus filter may allow.
const MAX_FOCUS_BOTS: usize = 200;

/// The stored filter for a `PUT /api/v1/notifications/focus` body, or the error code.
fn focus_from_body(body: &Value) -> Result<(String, Value), &'static str> {
    let device = body.get("deviceId").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty() && d.len() <= 200).ok_or("bad_value")?;
    let active = body.get("active").and_then(Value::as_bool).ok_or("bad_value")?;
    let allowed = match body.get("allowedBotIds") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Array(a)) if a.len() <= MAX_FOCUS_BOTS && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 200)) => Value::Array(a.clone()),
        _ => return Err("bad_value"),
    };
    let allow_approvals = match body.get("allowApprovals") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        _ => return Err("bad_value"),
    };
    Ok((device.to_string(), json!({ "active": active, "allowedBotIds": allowed, "allowApprovals": allow_approvals })))
}

/// Store `focus` on `user`'s device `device_id` and stamp it confirmed now. False when no such device.
pub async fn set_focus(db: &PgPool, user: &str, device_id: &str, focus: &Value) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("UPDATE apns_device_tokens SET focus = $3, focus_at = now() WHERE user_id = $1 AND device_id = $2").bind(user).bind(device_id).bind(focus).execute(db).await?;
    Ok(done.rows_affected() > 0)
}

async fn put_focus_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Option<Json<Value>>) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let (device, focus) = match body.as_ref().map(|b| focus_from_body(&b.0)) {
        Some(Ok(v)) => v,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_value" }))).into_response(),
    };
    match set_focus(&state.db, &user, &device, &focus).await {
        Ok(true) => Json(json!({ "ok": true, "deviceId": device, "focus": focus })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "unknown_device" }))).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Status snapshots and revoke
// ---------------------------------------------------------------------------

fn needs_you(snapshot: &Value) -> Vec<String> {
    snapshot.get("needsYou").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// What [`ingest_snapshot`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotIngest {
    /// Same content as the stored one: nothing changed.
    Unchanged,
    /// Stored. `revoke` holds the notification tags to remove from the person's iPhones (their
    /// `notification_delivered` rows are already deleted).
    Stored { revoke: Vec<String> },
}

/// Keep `snapshot` as `runtime_id`'s latest for `user`, and work out which delivered notifications to
/// revoke: those of every inbox id in the previous `needsYou` that is not in this one. Sends nothing;
/// the caller hands the tags to [`super::apns::revoke`].
pub async fn ingest_snapshot(db: &PgPool, user: &str, runtime_id: &str, snapshot: &Value) -> Result<SnapshotIngest, sqlx::Error> {
    let hash = super::apns::sha256_hex(snapshot.to_string().as_bytes());
    let mut tx = db.begin().await?;
    let previous: Option<(Value, String)> = sqlx::query_as("SELECT snapshot, hash FROM status_snapshots WHERE user_id = $1 AND runtime_id = $2 FOR UPDATE").bind(user).bind(runtime_id).fetch_optional(&mut *tx).await?;
    if previous.as_ref().is_some_and(|(_, h)| *h == hash) {
        return Ok(SnapshotIngest::Unchanged);
    }
    sqlx::query(
        "INSERT INTO status_snapshots (user_id, runtime_id, snapshot, hash, updated_at) VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (user_id, runtime_id) DO UPDATE SET snapshot = EXCLUDED.snapshot, hash = EXCLUDED.hash, updated_at = now()",
    )
    .bind(user)
    .bind(runtime_id)
    .bind(snapshot)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;
    let now = needs_you(snapshot);
    let left: Vec<String> = previous.map(|(p, _)| needs_you(&p)).unwrap_or_default().into_iter().filter(|id| !now.contains(id)).collect();
    let mut revoke = Vec::new();
    if !left.is_empty() {
        revoke = sqlx::query_scalar("DELETE FROM notification_delivered WHERE user_id = $1 AND inbox_id = ANY($2) RETURNING tag").bind(user).bind(&left).fetch_all(&mut *tx).await?;
        revoke.sort();
        revoke.dedup();
    }
    tx.commit().await?;
    Ok(SnapshotIngest::Stored { revoke })
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
    let pref = preference(db, user, event).await;
    if !pref.unwrap_or_else(|| default_enabled(event)) {
        return Outcome::Off;
    }
    if event == "agent.run.completed" && pref.is_none() && !run_worth_pushing(data) {
        return Outcome::Skipped;
    }
    let Some(mut msg) = message_for(event, data) else { return Outcome::Skipped };
    if let Some(d) = msg.data.as_object_mut() {
        d.insert("eventId".into(), json!(event_id));
    }
    // Coding-session permission asks: browsers get them from the remote-control lane, so only iPhones here.
    let apns_only = event == "approval.requested" && s(data, &["source"]) == Some("gizzi-permission");
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
    let sent = if apns_only { super::apns::send_to_user(db, user, &msg).await } else { web_push::send_to_user(db, transport, vapid, user, &msg).await };
    if sent > 0 {
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
        for sql in [include_str!("../../migrations_pg/039_push_subscriptions.sql"), include_str!("../../migrations_pg/060_notification_preferences.sql"), include_str!("../../migrations_pg/102_notification_quiet_hours.sql"), include_str!("../../migrations_pg/107_quiet_hours_levels.sql"), include_str!("../../migrations_pg/111_apns_device_tokens.sql"), include_str!("../../migrations_pg/120_ios_system_surfaces.sql")] {
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
        for (name, on) in [("approval.requested", true), ("thread.needs_user", true), ("call.ended", true), ("subscription.login_needed", true), ("message.received", true), ("agent.run.completed", true)] {
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
        // Coding-session permission asks have copy too (the sink sends them to iPhones only).
        assert_eq!(message_for("approval.requested", &json!({ "approvalId": "x", "source": "gizzi-permission" })).unwrap().tag, "approval-x");

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
        assert_eq!((m.title.as_str(), m.body.as_str(), m.tag.as_str(), m.url.as_str()), ("Something needs fixing", "Morning briefing failed: the bot is signed out.", "attention-routine_failed-r1", "/?bot=b1"));
        let fixed = message_for("attention.resolved", &json!({ "attentionId": "routine_failed:r1" })).unwrap();
        assert_eq!(fixed.tag, m.tag, "resolved replaces opened");
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
        assert_eq!((row(&v, "approval.requested")["enabled"].clone(), row(&v, "inbox.item.created")["enabled"].clone()), (json!(true), json!(false)));
        let changes = json!({ "approval.requested": false, "inbox.item.created": true });
        set_preferences(&db, "u1", changes.as_object().unwrap()).await.unwrap();
        let v = preferences(&db, "u1").await.unwrap();
        assert_eq!(row(&v, "approval.requested")["enabled"], false);
        assert_eq!(row(&v, "approval.requested")["default"], true);
        assert_eq!(row(&v, "inbox.item.created")["enabled"], true);
        assert!(enabled(&db, "u2", "approval.requested").await, "per user");
        set_preferences(&db, "u1", json!({ "approval.requested": null }).as_object().unwrap()).await.unwrap();
        assert!(enabled(&db, "u1", "approval.requested").await, "null = back to default");
        // All or nothing, registry names only.
        assert_eq!(set_preferences(&db, "u1", json!({ "inbox.item.created": false, "nope": true }).as_object().unwrap()).await, Err(PrefError::Bad("unknown_event")));
        assert!(enabled(&db, "u1", "inbox.item.created").await, "the bad batch changed nothing");
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
        assert_eq!(outcome(&db, &quiet).await, "skipped", "a short successful run does not push by default");
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

        // Failed, or two minutes or longer: pushes by default.
        let failed = emit(&db, "u1", "agent.run.completed", json!({ "run_id": "r3", "status": "failed", "durationMs": 4000 })).await;
        let long = emit(&db, "u1", "agent.run.completed", json!({ "run_id": "r4", "status": "completed", "durationMs": 120_000 })).await;
        let short = emit(&db, "u1", "agent.run.completed", json!({ "run_id": "r5", "status": "completed", "durationMs": 119_000 })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!((outcome(&db, &failed).await.as_str(), outcome(&db, &long).await.as_str(), outcome(&db, &short).await.as_str()), ("sent", "sent", "skipped"));
        // Coding-session permission asks: the remote-control lane sends their Web Push, so the sink sends none.
        let before = rec.sent.lock().unwrap().len();
        let gizzi = emit(&db, "u1", "approval.requested", json!({ "approvalId": "perm1", "source": "gizzi-permission", "summary": "run tests" })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &gizzi).await, "unreachable", "no iPhone registered");
        assert_eq!(rec.sent.lock().unwrap().len(), before, "no second Web Push");
        // Off wins over the rule.
        set_preferences(&db, "u2", json!({ "agent.run.completed": false }).as_object().unwrap()).await.unwrap();
        let u2_failed = emit(&db, "u2", "agent.run.completed", json!({ "status": "failed" })).await;
        sink_once(&db, &rec, &vapid).await.unwrap();
        assert_eq!(outcome(&db, &u2_failed).await, "off");

        // Turned on explicitly: every finished run pushes.
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

    #[test]
    fn category_comes_from_the_inbox_kind() {
        let cat = |event: &str, data: Value| message_for(event, &data).unwrap().data["category"].as_str().map(str::to_string);
        let fits = json!({ "inboxId": "approval:a1", "approveInline": true, "contentHash": "h1" });
        for kind in APPROVAL_KINDS {
            let mut d = fits.clone();
            d["inboxKind"] = json!(kind);
            assert_eq!(cat("approval.requested", d.clone()).as_deref(), Some("APPROVAL_INLINE"), "{kind}");
            // An email draft arrives as thread.needs_user and still gets approval buttons.
            assert_eq!(cat("thread.needs_user", d.clone()).as_deref(), Some("APPROVAL_INLINE"), "{kind} as needs_user");
            // Too long to show (no approveInline), or no hash to echo: Review.
            let mut long = d.clone();
            long["approveInline"] = json!(false);
            assert_eq!(cat("approval.requested", long).as_deref(), Some("APPROVAL_REVIEW"), "{kind} long");
            let mut nohash = d.clone();
            nohash.as_object_mut().unwrap().remove("contentHash");
            assert_eq!(cat("approval.requested", nohash).as_deref(), Some("APPROVAL_REVIEW"), "{kind} no hash");
            // Nothing to act on without an inbox id.
            let mut noid = d.clone();
            noid.as_object_mut().unwrap().remove("inboxId");
            assert_eq!(cat("approval.requested", noid), None, "{kind} no id");
        }
        // The email draft's link opens the inbox item, and its tag is the item.
        let m = message_for("thread.needs_user", &json!({ "inboxId": "email:e1", "inboxKind": "email_draft", "botId": "b1", "threadId": "t1" })).unwrap();
        assert_eq!((m.url.as_str(), m.tag.as_str()), ("/?allternit_inbox=email%3Ae1", "email-e1"));
        assert_eq!(cat("message.received", json!({ "senderKind": "bot" })).as_deref(), Some("BOT_REPLY"));
        assert_eq!(cat("message.received", json!({ "senderKind": "person" })), None, "a person's message has no Reply button");
        assert_eq!(cat("thread.needs_user", json!({})).as_deref(), Some("BOT_REPLY"));
        assert_eq!(cat("agent.run.completed", json!({ "status": "failed" })).as_deref(), Some("RUN_DONE"));
        let m = message_for("attention.opened", &json!({ "attentionId": "at9" })).unwrap();
        assert_eq!((m.data["category"].as_str(), m.data["inboxId"].as_str(), m.tag.as_str()), (Some("NEEDS_FIX"), Some("attention:at9"), "attention-at9"));
        assert_eq!(cat("usage.threshold", json!({ "meter": "plan" })), None);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn the_focus_filter_mutes_other_bots_and_lets_approvals_through() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::routes::apns::{self, device_targets, payload};
        std::env::set_var(crate::auth::dev_token::ALLOW_DEV_TOKEN_ENV, "true");
        std::env::set_var("APNS_KEY_P8", apns::tests::fixture_pem());
        std::env::set_var("APNS_KEY_ID", "KEY1234567");
        std::env::set_var("APNS_TEAM_ID", "TEAM123456");
        let state = crate::routes::test_support::test_state(Arc::new(crate::routes::test_support::MockGateway::new(None, vec![]))).await;
        let db = state.db.clone();
        schema(&db).await;
        let app = Router::new().merge(apns::routes()).merge(routes()).with_state(state);
        let call = |method: &'static str, path: &'static str, body: Value| {
            let app = app.clone();
            async move {
                let req = Request::builder().method(method).uri(path).header("content-type", "application/json").header("authorization", "Bearer dev-api-token").body(Body::from(body.to_string())).unwrap();
                app.oneshot(req).await.unwrap().status()
            }
        };
        let token = "f".repeat(64);
        assert_eq!(call("POST", "/api/v1/device-tokens", json!({ "token": token, "platform": "ios", "deviceId": "phone-1", "capabilities": ["actions", "nse"] })).await, StatusCode::CREATED);
        assert_eq!(call("PUT", "/api/v1/notifications/focus", json!({ "deviceId": "phone-1", "active": true, "allowedBotIds": ["b-work"], "allowApprovals": true })).await, StatusCode::OK);
        assert_eq!(call("PUT", "/api/v1/notifications/focus", json!({ "deviceId": "nope", "active": true, "allowedBotIds": null })).await, StatusCode::NOT_FOUND);
        assert_eq!(call("PUT", "/api/v1/notifications/focus", json!({ "deviceId": "phone-1", "active": "yes" })).await, StatusCode::BAD_REQUEST);

        let target = device_targets(&db, "dev-user").await.unwrap().remove(0);
        let other = message_for("message.received", &json!({ "botId": "b-home", "from": "Olive", "text": "hi", "senderKind": "bot" })).unwrap();
        let p = payload(&other, &target);
        assert_eq!(p["aps"]["interruption-level"], "passive", "another bot's alert is muted, not dropped: {p}");
        assert!(p["aps"].get("sound").is_none());
        let allowed = message_for("message.received", &json!({ "botId": "b-work", "from": "Raven", "text": "hi", "senderKind": "bot" })).unwrap();
        assert!(payload(&allowed, &target)["aps"].get("interruption-level").is_none(), "an allowed bot keeps its level");
        let ask = message_for("approval.requested", &json!({ "botId": "b-home", "approvalId": "ap1", "inboxId": "approval:ap1", "inboxKind": "approval" })).unwrap();
        let p = payload(&ask, &target);
        assert_eq!((p["aps"]["interruption-level"].as_str(), p["aps"]["sound"].as_str()), (Some("time-sensitive"), Some("default")), "approvals pass: {p}");

        // Approvals not allowed: muted too. A filter not confirmed for a day no longer applies.
        assert_eq!(call("PUT", "/api/v1/notifications/focus", json!({ "deviceId": "phone-1", "active": true, "allowedBotIds": null, "allowApprovals": false })).await, StatusCode::OK);
        let target = device_targets(&db, "dev-user").await.unwrap().remove(0);
        assert_eq!(payload(&ask, &target)["aps"]["interruption-level"], "passive");
        sqlx::query("UPDATE apns_device_tokens SET focus_at = now() - interval '25 hours'").execute(&db).await.unwrap();
        let target = device_targets(&db, "dev-user").await.unwrap().remove(0);
        assert_eq!(payload(&other, &target)["aps"]["sound"], "default", "stale filter");
    }

    #[tokio::test]
    async fn a_snapshot_revokes_what_left_needs_you() {
        use crate::routes::runtime_events::ingest;
        let db = test_pool().await;
        schema(&db).await;
        let snap = |id: &str, needs: Value| json!({ "id": id, "type": "status.snapshot", "data": { "pendingApprovals": 2, "needsYou": needs } });
        let r = ingest(&db, "u1", "rt_1", vec![snap("s1", json!(["approval:ap1", "attention:at9"]))]).await.unwrap();
        assert_eq!(r["results"][0]["status"], "accepted");
        // Never a backbone event, so never a notification.
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM platform_events").fetch_one(&db).await.unwrap(), 0);
        crate::routes::apns::record_delivered(&db, "u1", "approval:ap1", "approval-ap1", Some("e1")).await;
        crate::routes::apns::record_delivered(&db, "u1", "attention:at9", "attention-at9", Some("e2")).await;
        crate::routes::apns::record_delivered(&db, "u2", "approval:ap1", "approval-ap1", Some("e3")).await;

        // Same content again: unchanged.
        let r = ingest(&db, "u1", "rt_1", vec![snap("s2", json!(["approval:ap1", "attention:at9"]))]).await.unwrap();
        assert_eq!(r["results"][0]["status"], "duplicate");
        // ap1 was approved on another device: only its tag is revoked, only for this person.
        assert_eq!(
            ingest_snapshot(&db, "u1", "rt_1", &json!({ "pendingApprovals": 1, "needsYou": ["attention:at9"] })).await.unwrap(),
            SnapshotIngest::Stored { revoke: vec!["approval-ap1".into()] }
        );
        let left: Vec<(String, String)> = sqlx::query_as("SELECT user_id, inbox_id FROM notification_delivered ORDER BY user_id").fetch_all(&db).await.unwrap();
        assert_eq!(left, vec![("u1".into(), "attention:at9".into()), ("u2".into(), "approval:ap1".into())]);
        // Another runtime's snapshot has its own previous state.
        assert_eq!(ingest_snapshot(&db, "u1", "rt_2", &json!({ "needsYou": [] })).await.unwrap(), SnapshotIngest::Stored { revoke: vec![] });
        // Every forwarded event carries the runtime cloud authenticated.
        ingest(&db, "u1", "rt_1", vec![json!({ "id": "e9", "type": "approval.requested", "data": { "approvalId": "ap2", "runtimeId": "spoofed" } })]).await.unwrap();
        let data: Value = sqlx::query_scalar("SELECT data FROM platform_events").fetch_one(&db).await.unwrap();
        assert_eq!(data["runtimeId"], "rt_1");
    }

    /// The full router builds (no axum route-overlap panic) with these routes in it.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_full_router_builds() {
        let state = crate::routes::test_support::test_state(Arc::new(crate::routes::test_support::MockGateway::new(None, vec![]))).await;
        let _ = crate::create_router(state);
    }
}
