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
];

/// Only look at events stored this recently (the sink polls every few seconds).
const SCAN_WINDOW_MINUTES: i64 = 10;
/// Don't push an event that happened at its producer longer ago than this.
pub const STALE_AFTER_HOURS: i64 = 12;
/// A `message.received` this soon after a direct channel push for the same person is the same message.
pub const DIRECT_DEDUPE_SECS: i64 = 120;
const POLL: Duration = Duration::from_secs(3);
const BATCH: i64 = 200;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/notifications/preferences", get(get_route).put(put_route))
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
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Sent => "sent",
            Outcome::Off => "off",
            Outcome::Unreachable => "unreachable",
            Outcome::Skipped => "skipped",
        }
    }
}

type EventRow = (String, String, String, Value, Option<DateTime<Utc>>);

async fn decide_and_send(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid, row: &EventRow) -> Outcome {
    let (_, user, event, data, occurred_at) = row;
    if occurred_at.is_some_and(|t| Utc::now() - t > chrono::Duration::hours(STALE_AFTER_HOURS)) {
        return Outcome::Skipped;
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
        if direct || !web_push::allow_message_push(db, user, &msg.tag).await {
            return Outcome::Skipped;
        }
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
            if passes % 1200 == 0 {
                let _ = sqlx::query("DELETE FROM notification_push_log WHERE created_at < now() - interval '2 days'").execute(&db).await;
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
        for sql in [include_str!("../../migrations_pg/039_push_subscriptions.sql"), include_str!("../../migrations_pg/060_notification_preferences.sql")] {
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
