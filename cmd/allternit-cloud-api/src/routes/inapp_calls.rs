//! Free messages and calls between Allternit users who are linked through an invite.
//! Migration `038_inapp_calls.sql`.
//!
//! A *link* is an `invite_contacts` row (035), in either direction: the owner of a bot
//! and a person who joined through that bot's invite. Nobody else can message or ring.
//!
//! All routes take a Clerk session or a `compute`-scoped API key:
//! - `POST /api/v1/phone/contacts/:userId/thread` {botId}            → `{threadId, created}`
//! - `GET  /api/v1/phone/threads/:id/events?after=&limit=`           → `{events:[…]}` (both people read the same log)
//! - `POST /api/v1/phone/threads/:id/messages` {text}                → `{event}`
//! - `POST /api/v1/phone/contacts/:userId/ring` {botId, mode:"voice"} → `{callId, threadId, room, token, url, state, expiresAt}`
//! - `POST /api/v1/phone/contacts/:userId/ring/:room/answer`         → `{callId, threadId, room, token, url}` (callee)
//! - `POST /api/v1/phone/contacts/:userId/ring/:room/decline`        → `{ok}` (callee)
//! - `POST /api/v1/phone/contacts/:userId/ring/:room/cancel`         → `{ok}` (caller)
//! - `GET  /api/v1/phone/incoming`                                   → `{calls:[…]}`; the app polls this every few
//!   seconds, and each poll is also its presence heartbeat (`POST /api/v1/phone/presence` is the same heartbeat alone)
//! - `PUT  /api/v1/phone/call-prefs` {autoAnswerBotId}               the voice agent answers rings for that bot
//!
//! `:userId` is always the other person. Ring errors: 403 `not_a_contact`, 409 `callee_offline`
//! ("We'll let them know you called", a missed-call event lands in the thread), 409 `callee_busy`,
//! 429 `too_many_rings`, 503 `livekit_not_configured`.
//!
//! With Web Push configured (`web_push`), a callee whose app is closed but who has a device registered is
//! rung by push instead of getting an instant missed call; a ring nobody answers, and a cancelled one,
//! become a "Missed call" push that replaces the ringing notification, and a thread message pushes the
//! other person (one per thread per 20 s).
//!
//! The thread log and the call state live in the cloud, so a sleeping runtime never blocks a ring.
//! Rooms are `call-app-<uuid>`; LiveKit creates them when the caller joins, or up front with the
//! `allternit-voice` agent when the callee has auto-answer on.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

use super::livekit_admin::{LiveKitAdminClient, LiveKitConfig, LiveKitError, LiveKitHttpAdmin, SIP_AGENT_NAME};
use super::phone::PhoneError;
use super::web_push::{self, PushMessage};
use crate::{ApiError, ApiState};

/// A person counts as online when their app asked for incoming calls this recently.
const PRESENCE_SECS: i64 = 60;
/// How long a ring lasts before it becomes a missed call.
const RING_SECS: i64 = 45;
/// Rings from one person to one contact in ten minutes.
const MAX_RINGS_PER_10_MIN: i64 = 5;
const MAX_MESSAGE_CHARS: usize = 4000;
const EVENT_PAGE: i64 = 200;
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const ROOM_PREFIX: &str = "call-app-";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/phone/contacts/:userId/thread", post(thread_route))
        .route("/api/v1/phone/contacts/:userId/ring", post(ring_route))
        .route("/api/v1/phone/contacts/:userId/ring/:room/answer", post(answer_route))
        .route("/api/v1/phone/contacts/:userId/ring/:room/decline", post(decline_route))
        .route("/api/v1/phone/contacts/:userId/ring/:room/cancel", post(cancel_route))
        .route("/api/v1/phone/threads/:id/events", get(events_route))
        .route("/api/v1/phone/threads/:id/messages", post(message_route))
        .route("/api/v1/phone/incoming", get(incoming_route))
        .route("/api/v1/phone/presence", post(presence_route))
        .route("/api/v1/phone/call-prefs", put(prefs_route))
}

/// Mark rings nobody picked up as missed (and tell the thread) every few seconds.
pub fn start_inapp_calls_worker(state: Arc<ApiState>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = expire_stale(&state.db).await {
                tracing::warn!("inapp calls sweep: {error:?}");
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CallError {
    Phone(PhoneError),
    /// 409: the callee has no app open. A missed-call event is already in the thread.
    Offline { thread_id: String },
    /// 410: the ring is over (answered elsewhere, cancelled, missed).
    Gone(&'static str),
    LiveKit(LiveKitError),
}

impl<E: Into<PhoneError>> From<E> for CallError {
    fn from(e: E) -> Self {
        Self::Phone(e.into())
    }
}

impl IntoResponse for CallError {
    fn into_response(self) -> Response {
        match self {
            Self::Phone(e) => e.into_response(),
            Self::Offline { thread_id } => (
                StatusCode::CONFLICT,
                Json(json!({ "error": "callee_offline", "message": "We'll let them know you called", "threadId": thread_id })),
            )
                .into_response(),
            Self::Gone(code) => (StatusCode::GONE, Json(json!({ "error": code }))).into_response(),
            Self::LiveKit(LiveKitError::NotConfigured) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "livekit_not_configured" }))).into_response(),
            Self::LiveKit(LiveKitError::PublicUrlMissing) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "livekit_public_url_not_configured" }))).into_response(),
            Self::LiveKit(other) => {
                tracing::warn!("inapp call livekit error: {other}");
                (StatusCode::BAD_GATEWAY, Json(json!({ "error": "livekit_error" }))).into_response()
            }
        }
    }
}

type CResult<T> = Result<T, CallError>;

fn bad(msg: &str) -> CallError {
    CallError::Phone(PhoneError::BadRequest(msg.into()))
}

// ---------------------------------------------------------------------------
// Links, threads, events
// ---------------------------------------------------------------------------

/// Who owns the bot and who joined through its invite.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Link {
    pub owner_user_id: String,
    pub bot_id: String,
    pub contact_user_id: String,
    pub label: String,
}

/// The invite link between `me` and `other`, or 403 `not_a_contact`. `bot_id` narrows the match
/// (a blank one accepts any bot the two share, which is how a contact rings an owner).
pub async fn link_between(db: &PgPool, me: &str, other: &str, bot_id: &str) -> CResult<Link> {
    if me == other {
        return Err(bad("you can't message or call yourself"));
    }
    let row: Option<Link> = sqlx::query_as(
        "SELECT owner_user_id, bot_id, contact_user_id, label FROM invite_contacts
          WHERE ((owner_user_id = $1 AND contact_user_id = $2) OR (owner_user_id = $2 AND contact_user_id = $1))
            AND ($3 = '' OR bot_id = $3)
          ORDER BY created_at LIMIT 1",
    )
    .bind(me)
    .bind(other)
    .bind(bot_id.trim())
    .fetch_optional(db)
    .await?;
    row.ok_or(CallError::Phone(PhoneError::Forbidden("not_a_contact")))
}

/// The thread for a link, created on first use. The same id comes back for both people.
pub async fn thread_for(db: &PgPool, link: &Link) -> CResult<(String, bool)> {
    let id = format!("dm_{}", uuid::Uuid::new_v4().simple());
    let inserted: Option<String> = sqlx::query_scalar(
        "INSERT INTO inapp_threads (id, owner_user_id, bot_id, contact_user_id) VALUES ($1, $2, $3, $4)
         ON CONFLICT (owner_user_id, bot_id, contact_user_id) DO NOTHING RETURNING id",
    )
    .bind(&id)
    .bind(&link.owner_user_id)
    .bind(&link.bot_id)
    .bind(&link.contact_user_id)
    .fetch_optional(db)
    .await?;
    if let Some(id) = inserted {
        return Ok((id, true));
    }
    let existing: String = sqlx::query_scalar("SELECT id FROM inapp_threads WHERE owner_user_id = $1 AND bot_id = $2 AND contact_user_id = $3")
        .bind(&link.owner_user_id)
        .bind(&link.bot_id)
        .bind(&link.contact_user_id)
        .fetch_one(db)
        .await?;
    Ok((existing, false))
}

async fn push_event(db: &PgPool, thread_id: &str, kind: &str, sender: &str, body: &str, room: Option<&str>) -> CResult<Value> {
    let row: (i64, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("INSERT INTO inapp_thread_events (thread_id, kind, sender_user_id, body, call_room) VALUES ($1, $2, $3, $4, $5) RETURNING id, created_at")
            .bind(thread_id)
            .bind(kind)
            .bind(sender)
            .bind(body)
            .bind(room)
            .fetch_one(db)
            .await?;
    Ok(json!({ "id": row.0, "threadId": thread_id, "kind": kind, "senderUserId": sender, "body": body, "room": room, "createdAt": row.1 }))
}

/// The thread, if `me` is one of its two people. Anyone else gets the same 404 as a missing thread.
async fn thread_member(db: &PgPool, me: &str, thread_id: &str) -> CResult<()> {
    let ok: Option<i32> = sqlx::query_scalar("SELECT 1 FROM inapp_threads WHERE id = $1 AND (owner_user_id = $2 OR contact_user_id = $2)")
        .bind(thread_id)
        .bind(me)
        .fetch_optional(db)
        .await?;
    ok.map(|_| ()).ok_or(CallError::Phone(PhoneError::NotFound("thread_not_found")))
}

pub async fn list_events(db: &PgPool, me: &str, thread_id: &str, after: i64, limit: i64) -> CResult<Value> {
    thread_member(db, me, thread_id).await?;
    let rows: Vec<(i64, String, String, String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, kind, sender_user_id, body, call_room, created_at FROM inapp_thread_events WHERE thread_id = $1 AND id > $2 ORDER BY id LIMIT $3",
    )
    .bind(thread_id)
    .bind(after)
    .bind(limit.clamp(1, EVENT_PAGE))
    .fetch_all(db)
    .await?;
    let events: Vec<Value> = rows
        .into_iter()
        .map(|(id, kind, sender, body, room, at)| json!({ "id": id, "threadId": thread_id, "kind": kind, "senderUserId": sender, "body": body, "room": room, "createdAt": at }))
        .collect();
    Ok(json!({ "events": events }))
}

pub async fn send_message(db: &PgPool, me: &str, thread_id: &str, text: &str) -> CResult<Value> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_MESSAGE_CHARS {
        return Err(bad("text must be 1 to 4000 characters"));
    }
    thread_member(db, me, thread_id).await?;
    let event = push_event(db, thread_id, "message", me, text, None).await?;
    notify_message(db, me, thread_id, text).await;
    Ok(json!({ "event": event }))
}

/// Push the other person in the thread about a new message. Never fails the send.
async fn notify_message(db: &PgPool, me: &str, thread_id: &str, text: &str) {
    let row: Option<(String, String, String)> = sqlx::query_as("SELECT owner_user_id, bot_id, contact_user_id FROM inapp_threads WHERE id = $1").bind(thread_id).fetch_optional(db).await.ok().flatten();
    let Some((owner, bot_id, contact)) = row else { return };
    let other = if owner == me { contact.clone() } else { owner.clone() };
    let label = match link_between(db, &other, me, &bot_id).await {
        Ok(link) => display_name(db, &other, &link, me).await,
        Err(_) => "Someone".to_string(),
    };
    web_push::spawn_notify_pref(
        db.clone(),
        other,
        "message.received",
        PushMessage {
            kind: "message",
            title: label,
            body: text.to_string(),
            url: format!("/?allternit_thread={thread_id}&bot={bot_id}&from={me}"),
            tag: format!("dm-{thread_id}"),
            data: json!({ "threadId": thread_id, "botId": bot_id, "from": me }),
            ttl_secs: 24 * 3600,
            high_urgency: false,
        },
        true,
    );
}

/// The "Incoming call" push for a ring, the same tag as its later "Missed call" so one replaces the other.
async fn notify_ring(db: &PgPool, call: &CallRow) {
    let label = caller_label(db, call).await;
    web_push::spawn_notify(
        db.clone(),
        call.callee_user_id.clone(),
        PushMessage {
            kind: "call",
            title: format!("Incoming call from {label}"),
            body: "Tap to answer".to_string(),
            url: format!("/?allternit_call={}&from={}&thread={}&bot={}", call.room, call.caller_user_id, call.thread_id, call.bot_id),
            tag: format!("call-{}", call.room),
            data: json!({ "room": call.room, "from": call.caller_user_id, "threadId": call.thread_id, "botId": call.bot_id, "label": label, "expiresAt": call.expires_at }),
            ttl_secs: RING_SECS as u32,
            high_urgency: true,
        },
        false,
    );
}

/// "Missed call from X" for the callee; replaces the ringing notification (same tag).
async fn notify_missed(db: &PgPool, call: &CallRow) {
    let label = caller_label(db, call).await;
    web_push::spawn_notify_pref(
        db.clone(),
        call.callee_user_id.clone(),
        "call.ended",
        PushMessage {
            kind: "missed_call",
            title: format!("Missed call from {label}"),
            body: "Tap to open the conversation".to_string(),
            url: format!("/?allternit_thread={}&bot={}&from={}", call.thread_id, call.bot_id, call.caller_user_id),
            tag: format!("call-{}", call.room),
            data: json!({ "room": call.room, "from": call.caller_user_id, "threadId": call.thread_id, "botId": call.bot_id, "label": label }),
            ttl_secs: 24 * 3600,
            high_urgency: false,
        },
        false,
    );
}

async fn caller_label(db: &PgPool, call: &CallRow) -> String {
    match link_between(db, &call.callee_user_id, &call.caller_user_id, &call.bot_id).await {
        Ok(link) => display_name(db, &call.callee_user_id, &link, &call.caller_user_id).await,
        Err(_) => "Someone".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

pub async fn heartbeat(db: &PgPool, user: &str) -> CResult<()> {
    sqlx::query("INSERT INTO phone_presence (user_id, last_seen_at) VALUES ($1, now()) ON CONFLICT (user_id) DO UPDATE SET last_seen_at = now()")
        .bind(user)
        .execute(db)
        .await?;
    Ok(())
}

/// Online = their app polled, or a paired runtime checked in, within the last 60 seconds.
pub async fn is_online(db: &PgPool, user: &str) -> CResult<bool> {
    let online: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM phone_presence WHERE user_id = $1 AND last_seen_at > now() - make_interval(secs => $2))
             OR EXISTS (SELECT 1 FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL AND status = 'online' AND last_seen_at > now() - make_interval(secs => $2))",
    )
    .bind(user)
    .bind(PRESENCE_SECS as f64)
    .fetch_one(db)
    .await?;
    Ok(online)
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct CallRow {
    room: String,
    thread_id: String,
    caller_user_id: String,
    callee_user_id: String,
    bot_id: String,
    state: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

const CALL_COLS: &str = "room, thread_id, caller_user_id, callee_user_id, bot_id, state, expires_at";

/// Rings nobody answered in time become missed calls; the thread gets one `call.missed` each.
pub async fn expire_stale(db: &PgPool) -> CResult<usize> {
    let rows: Vec<CallRow> = sqlx::query_as(&format!("UPDATE inapp_calls SET state = 'missed', ended_at = now() WHERE state = 'ringing' AND expires_at <= now() RETURNING {CALL_COLS}"))
        .fetch_all(db)
        .await?;
    for call in &rows {
        push_event(db, &call.thread_id, "call.missed", &call.caller_user_id, "", Some(&call.room)).await?;
        notify_missed(db, call).await;
    }
    Ok(rows.len())
}

fn access_json(lk: &dyn LiveKitAdminClient, room: &str, user: &str) -> CResult<(String, String)> {
    let access = lk.participant_access(room, &format!("user-{user}"), true).map_err(CallError::LiveKit)?;
    Ok((access.token, access.url))
}

/// Ring `other`. `bot_id` is the owner's bot (blank when a contact rings the owner).
pub async fn ring(db: &PgPool, lk: &dyn LiveKitAdminClient, me: &str, other: &str, bot_id: &str, mode: &str) -> CResult<Value> {
    if mode != "voice" {
        return Err(bad("mode must be \"voice\""));
    }
    let link = link_between(db, me, other, bot_id).await?;
    let (thread_id, _) = thread_for(db, &link).await?;
    expire_stale(db).await?;

    let too_many: i64 = sqlx::query_scalar("SELECT count(*) FROM inapp_calls WHERE caller_user_id = $1 AND callee_user_id = $2 AND created_at > now() - interval '10 minutes'")
        .bind(me)
        .bind(other)
        .fetch_one(db)
        .await?;
    if too_many >= MAX_RINGS_PER_10_MIN {
        return Err(PhoneError::TooMany("too_many_rings").into());
    }
    let room = format!("{ROOM_PREFIX}{}", uuid::Uuid::new_v4().simple());

    // The callee lets a bot of theirs answer: the voice agent joins the room, nobody rings.
    let auto_bot: Option<String> = sqlx::query_scalar("SELECT auto_answer_bot_id FROM phone_call_prefs WHERE user_id = $1").bind(other).fetch_optional(db).await?.flatten();
    if let Some(agent_bot) = auto_bot.filter(|b| !b.is_empty()) {
        let metadata = json!({ "botId": agent_bot, "ownerId": other, "callerUserId": me, "source": "inapp", "direction": "inbound" }).to_string();
        lk.create_room_with_agent(&room, SIP_AGENT_NAME, &metadata).await.map_err(CallError::LiveKit)?;
        sqlx::query("INSERT INTO inapp_calls (room, thread_id, caller_user_id, callee_user_id, bot_id, state, answered_by_agent, expires_at) VALUES ($1, $2, $3, $4, $5, 'answered', true, now())")
            .bind(&room)
            .bind(&thread_id)
            .bind(me)
            .bind(other)
            .bind(&link.bot_id)
            .execute(db)
            .await?;
        push_event(db, &thread_id, "call.answered", other, "agent", Some(&room)).await?;
        let (token, url) = access_json(lk, &room, me)?;
        return Ok(json!({ "callId": room, "threadId": thread_id, "room": room, "token": token, "url": url, "state": "answered", "answeredBy": "agent" }));
    }

    // App closed: with a pushable device registered they still get rung; otherwise it's a missed call now.
    if !is_online(db, other).await? && !web_push::can_reach(db, other).await {
        sqlx::query("INSERT INTO inapp_calls (room, thread_id, caller_user_id, callee_user_id, bot_id, state, expires_at, ended_at) VALUES ($1, $2, $3, $4, $5, 'missed', now(), now())")
            .bind(&room)
            .bind(&thread_id)
            .bind(me)
            .bind(other)
            .bind(&link.bot_id)
            .execute(db)
            .await?;
        push_event(db, &thread_id, "call.missed", me, "", Some(&room)).await?;
        return Err(CallError::Offline { thread_id });
    }
    let busy: i64 = sqlx::query_scalar("SELECT count(*) FROM inapp_calls WHERE callee_user_id = $1 AND state = 'ringing'").bind(other).fetch_one(db).await?;
    if busy > 0 {
        return Err(PhoneError::Conflict("callee_busy").into());
    }
    let (token, url) = access_json(lk, &room, me)?;
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(RING_SECS);
    sqlx::query("INSERT INTO inapp_calls (room, thread_id, caller_user_id, callee_user_id, bot_id, expires_at) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(&room)
        .bind(&thread_id)
        .bind(me)
        .bind(other)
        .bind(&link.bot_id)
        .bind(expires_at)
        .execute(db)
        .await?;
    push_event(db, &thread_id, "call.ring", me, "", Some(&room)).await?;
    notify_ring(db, &CallRow { room: room.clone(), thread_id: thread_id.clone(), caller_user_id: me.to_string(), callee_user_id: other.to_string(), bot_id: link.bot_id.clone(), state: "ringing".into(), expires_at }).await;
    Ok(json!({ "callId": room, "threadId": thread_id, "room": room, "token": token, "url": url, "state": "ringing", "expiresAt": expires_at }))
}

/// Move a ringing call to `to` for the right person. `by_callee` picks who may do it.
async fn finish_ring(db: &PgPool, me: &str, other: &str, room: &str, to: &str, by_callee: bool) -> CResult<CallRow> {
    expire_stale(db).await?;
    let (mine, theirs) = if by_callee { ("callee_user_id", "caller_user_id") } else { ("caller_user_id", "callee_user_id") };
    let row: Option<CallRow> = sqlx::query_as(&format!(
        "UPDATE inapp_calls SET state = $1, ended_at = CASE WHEN $1 = 'answered' THEN NULL ELSE now() END
          WHERE room = $2 AND {mine} = $3 AND {theirs} = $4 AND state = 'ringing' AND expires_at > now() RETURNING {CALL_COLS}"
    ))
    .bind(to)
    .bind(room)
    .bind(me)
    .bind(other)
    .fetch_optional(db)
    .await?;
    if let Some(row) = row {
        return Ok(row);
    }
    let exists: Option<i32> = sqlx::query_scalar(&format!("SELECT 1 FROM inapp_calls WHERE room = $1 AND {mine} = $2 AND {theirs} = $3"))
        .bind(room)
        .bind(me)
        .bind(other)
        .fetch_optional(db)
        .await?;
    Err(if exists.is_some() { CallError::Gone("call_over") } else { CallError::Phone(PhoneError::NotFound("call_not_found")) })
}

pub async fn answer(db: &PgPool, lk: &dyn LiveKitAdminClient, me: &str, other: &str, room: &str) -> CResult<Value> {
    let call = finish_ring(db, me, other, room, "answered", true).await?;
    push_event(db, &call.thread_id, "call.answered", me, "", Some(room)).await?;
    let (token, url) = access_json(lk, room, me)?;
    Ok(json!({ "callId": room, "threadId": call.thread_id, "room": room, "token": token, "url": url }))
}

pub async fn decline(db: &PgPool, me: &str, other: &str, room: &str) -> CResult<Value> {
    let call = finish_ring(db, me, other, room, "declined", true).await?;
    push_event(db, &call.thread_id, "call.declined", me, "", Some(room)).await?;
    Ok(json!({ "ok": true }))
}

pub async fn cancel(db: &PgPool, me: &str, other: &str, room: &str) -> CResult<Value> {
    let call = finish_ring(db, me, other, room, "cancelled", false).await?;
    // A ring cancelled before they picked up is a missed call for the callee.
    push_event(db, &call.thread_id, "call.cancelled", me, "", Some(room)).await?;
    notify_missed(db, &call).await;
    Ok(json!({ "ok": true }))
}

async fn display_name(db: &PgPool, viewer: &str, link: &Link, who: &str) -> String {
    if viewer == link.owner_user_id {
        return link.label.clone();
    }
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = $1").bind(who).fetch_optional(db).await.ok().flatten().flatten();
    name.as_deref().and_then(|n| n.split_whitespace().next()).map(str::to_string).unwrap_or_else(|| "Someone".to_string())
}

/// Rings waiting for `me`. Asking is the presence heartbeat.
pub async fn incoming(db: &PgPool, me: &str) -> CResult<Value> {
    heartbeat(db, me).await?;
    expire_stale(db).await?;
    let rows: Vec<CallRow> = sqlx::query_as(&format!("SELECT {CALL_COLS} FROM inapp_calls WHERE callee_user_id = $1 AND state = 'ringing' ORDER BY created_at LIMIT 5"))
        .bind(me)
        .fetch_all(db)
        .await?;
    let mut calls = Vec::new();
    for c in rows {
        let link = link_between(db, me, &c.caller_user_id, &c.bot_id).await.ok();
        let from = match &link {
            Some(l) => display_name(db, me, l, &c.caller_user_id).await,
            None => "Someone".to_string(),
        };
        calls.push(json!({
            "callId": c.room, "room": c.room, "threadId": c.thread_id, "mode": "voice", "botId": c.bot_id,
            "from": { "userId": c.caller_user_id, "label": from }, "expiresAt": c.expires_at,
        }));
    }
    Ok(json!({ "calls": calls }))
}

/// Let one of the caller's own bots answer rings with the voice agent (`null` turns it off).
pub async fn set_auto_answer(db: &PgPool, me: &str, bot_id: Option<&str>) -> CResult<Value> {
    let bot = bot_id.map(str::trim).filter(|b| !b.is_empty());
    if let Some(bot) = bot {
        let owned: Option<i32> = sqlx::query_scalar("SELECT 1 FROM voice_bot_config WHERE bot_id = $1 AND user_id = $2").bind(bot).bind(me).fetch_optional(db).await?;
        if owned.is_none() {
            return Err(CallError::Phone(PhoneError::Forbidden("not_your_bot")));
        }
    }
    sqlx::query("INSERT INTO phone_call_prefs (user_id, auto_answer_bot_id) VALUES ($1, $2) ON CONFLICT (user_id) DO UPDATE SET auto_answer_bot_id = $2, updated_at = now()")
        .bind(me)
        .bind(bot)
        .execute(db)
        .await?;
    Ok(json!({ "autoAnswerBotId": bot }))
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

async fn user(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    crate::auth::resolve_user_scoped(&state.db, headers, "compute").await.map(|u| u.id)
}

fn livekit() -> CResult<LiveKitHttpAdmin> {
    LiveKitConfig::from_env().map(LiveKitHttpAdmin::new).ok_or(CallError::LiveKit(LiveKitError::NotConfigured))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BotBody {
    #[serde(default)]
    bot_id: String,
    mode: Option<String>,
}

async fn thread_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(other): Path<String>, Json(body): Json<BotBody>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        let link = link_between(&state.db, &me, &other, &body.bot_id).await?;
        let (thread_id, created) = thread_for(&state.db, &link).await?;
        Ok::<Response, CallError>(Json(json!({ "threadId": thread_id, "created": created })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn ring_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(other): Path<String>, Json(body): Json<BotBody>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        let lk = livekit()?;
        let out = ring(&state.db, &lk, &me, &other, &body.bot_id, body.mode.as_deref().unwrap_or("voice")).await?;
        Ok::<Response, CallError>(Json(out).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn answer_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path((other, room)): Path<(String, String)>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        let lk = livekit()?;
        Ok::<Response, CallError>(Json(answer(&state.db, &lk, &me, &other, &room).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn decline_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path((other, room)): Path<(String, String)>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<Response, CallError>(Json(decline(&state.db, &me, &other, &room).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn cancel_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path((other, room)): Path<(String, String)>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<Response, CallError>(Json(cancel(&state.db, &me, &other, &room).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
struct EventsQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

async fn events_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<Response, CallError>(Json(list_events(&state.db, &me, &id, q.after.unwrap_or(0), q.limit.unwrap_or(EVENT_PAGE)).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn message_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<MessageBody>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<Response, CallError>((StatusCode::CREATED, Json(send_message(&state.db, &me, &id, &body.text).await?)).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn incoming_route(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<Response, CallError>(Json(incoming(&state.db, &me).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn presence_route(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        heartbeat(&state.db, &me).await?;
        Ok::<Response, CallError>(Json(json!({ "ok": true })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrefsBody {
    auto_answer_bot_id: Option<String>,
}

async fn prefs_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<PrefsBody>) -> Response {
    let run = async {
        let me = user(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<Response, CallError>(Json(set_auto_answer(&state.db, &me, body.auto_answer_bot_id.as_deref()).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::livekit_admin::{CreateSipParticipantRequest, ParticipantAccess};
    use crate::routes::test_support::test_pool;
    use std::sync::Mutex;

    const OWNER: &str = "user_owner";
    const FRIEND: &str = "user_friend";
    const STRANGER: &str = "user_stranger";
    const BOT: &str = "bot_1";

    #[derive(Default)]
    struct FakeLiveKit {
        rooms: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl LiveKitAdminClient for FakeLiveKit {
        async fn ensure_inbound_trunk(&self, _n: &str, _e: &str) -> Result<String, LiveKitError> {
            unimplemented!()
        }
        async fn delete_inbound_trunk(&self, _t: &str) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        async fn ensure_dispatch_rule(&self, _t: &str, _n: &str, _b: &str, _o: &str, _to: &str) -> Result<String, LiveKitError> {
            unimplemented!()
        }
        async fn delete_dispatch_rule(&self, _r: &str) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        async fn create_sip_participant(&self, _r: CreateSipParticipantRequest) -> Result<Value, LiveKitError> {
            unimplemented!()
        }
        async fn create_room_with_agent(&self, room: &str, agent: &str, metadata: &str) -> Result<(), LiveKitError> {
            self.rooms.lock().unwrap().push((room.into(), agent.into(), metadata.into()));
            Ok(())
        }
        async fn send_data(&self, _r: &str, _t: &str, _p: &[u8]) -> Result<(), LiveKitError> {
            Ok(())
        }
        fn participant_access(&self, room: &str, identity: &str, can_publish: bool) -> Result<ParticipantAccess, LiveKitError> {
            Ok(ParticipantAccess { token: format!("tok:{room}:{identity}:{can_publish}"), url: "wss://livekit.test".into() })
        }
    }

    /// Schema-per-test pool with just the tables this module reads, plus the real 038 migration.
    async fn pool() -> PgPool {
        let db = test_pool().await;
        for sql in [
            "CREATE TABLE invite_contacts (invite_id TEXT PRIMARY KEY, owner_user_id TEXT NOT NULL, bot_id TEXT NOT NULL, contact_user_id TEXT NOT NULL, label TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now())",
            "CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT)",
            "CREATE TABLE voice_bot_config (bot_id TEXT PRIMARY KEY, user_id TEXT NOT NULL)",
        ] {
            sqlx::query(sql).execute(&db).await.unwrap();
        }
        // the test pool is a private schema, so the migration runs unqualified
        sqlx::raw_sql(&include_str!("../../migrations_pg/038_inapp_calls.sql").replace("public.", "")).execute(&db).await.unwrap();
        sqlx::raw_sql(&include_str!("../../migrations_pg/039_push_subscriptions.sql").replace("public.", "")).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO invite_contacts (invite_id, owner_user_id, bot_id, contact_user_id, label) VALUES ('inv1', $1, $2, $3, 'Sam')").bind(OWNER).bind(BOT).bind(FRIEND).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO users (id, name) VALUES ($1, 'Olive Owner'), ($2, 'Fran Friend')").bind(OWNER).bind(FRIEND).execute(&db).await.unwrap();
        db
    }

    async fn kinds(db: &PgPool, thread: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT kind FROM inapp_thread_events WHERE thread_id = $1 ORDER BY id").bind(thread).fetch_all(db).await.unwrap()
    }

    #[tokio::test]
    async fn only_linked_contacts_can_reach_each_other() {
        let db = pool().await;
        assert!(matches!(link_between(&db, OWNER, FRIEND, BOT).await, Ok(_)));
        // the other direction works, with or without the bot id
        assert!(link_between(&db, FRIEND, OWNER, BOT).await.is_ok());
        assert!(link_between(&db, FRIEND, OWNER, "").await.is_ok());
        // strangers, wrong bot, and self are refused
        assert!(matches!(link_between(&db, STRANGER, OWNER, BOT).await, Err(CallError::Phone(PhoneError::Forbidden("not_a_contact")))));
        assert!(matches!(link_between(&db, OWNER, STRANGER, BOT).await, Err(CallError::Phone(PhoneError::Forbidden("not_a_contact")))));
        assert!(matches!(link_between(&db, OWNER, FRIEND, "other_bot").await, Err(CallError::Phone(PhoneError::Forbidden("not_a_contact")))));
        assert!(matches!(link_between(&db, OWNER, OWNER, BOT).await, Err(CallError::Phone(PhoneError::BadRequest(_)))));
        let lk = FakeLiveKit::default();
        heartbeat(&db, OWNER).await.unwrap();
        assert!(matches!(ring(&db, &lk, STRANGER, OWNER, BOT, "voice").await, Err(CallError::Phone(PhoneError::Forbidden("not_a_contact")))));
    }

    #[tokio::test]
    async fn both_people_share_one_thread_and_its_messages() {
        let db = pool().await;
        let from_owner = thread_for(&db, &link_between(&db, OWNER, FRIEND, BOT).await.unwrap()).await.unwrap();
        let from_friend = thread_for(&db, &link_between(&db, FRIEND, OWNER, "").await.unwrap()).await.unwrap();
        assert!(from_owner.1, "first call creates it");
        assert!(!from_friend.1, "second call reuses it");
        assert_eq!(from_owner.0, from_friend.0, "same thread id for both sides");
        let id = from_owner.0;

        send_message(&db, OWNER, &id, "hi Fran").await.unwrap();
        send_message(&db, FRIEND, &id, "hi Olive").await.unwrap();
        for who in [OWNER, FRIEND] {
            let events = list_events(&db, who, &id, 0, 50).await.unwrap();
            let bodies: Vec<&str> = events["events"].as_array().unwrap().iter().map(|e| e["body"].as_str().unwrap()).collect();
            assert_eq!(bodies, ["hi Fran", "hi Olive"], "{who} sees the same log");
        }
        let after = list_events(&db, FRIEND, &id, 1, 50).await.unwrap();
        assert_eq!(after["events"].as_array().unwrap().len(), 1);
        // a third person can't read or write it, and can't tell it exists
        assert!(matches!(list_events(&db, STRANGER, &id, 0, 50).await, Err(CallError::Phone(PhoneError::NotFound("thread_not_found")))));
        assert!(matches!(send_message(&db, STRANGER, &id, "hello").await, Err(CallError::Phone(PhoneError::NotFound(_)))));
        assert!(send_message(&db, OWNER, &id, "   ").await.is_err());
    }

    #[tokio::test]
    async fn presence_window_is_sixty_seconds() {
        let db = pool().await;
        assert!(!is_online(&db, FRIEND).await.unwrap());
        heartbeat(&db, FRIEND).await.unwrap();
        assert!(is_online(&db, FRIEND).await.unwrap());
        sqlx::query("UPDATE phone_presence SET last_seen_at = now() - interval '61 seconds' WHERE user_id = $1").bind(FRIEND).execute(&db).await.unwrap();
        assert!(!is_online(&db, FRIEND).await.unwrap());
        // a paired runtime that checked in recently counts too
        sqlx::query("INSERT INTO runtime_devices (id, user_id, status, last_seen_at) VALUES ('rd1', $1, 'online', now())").bind(FRIEND).execute(&db).await.unwrap();
        assert!(is_online(&db, FRIEND).await.unwrap());
        sqlx::query("UPDATE runtime_devices SET revoked_at = now()").execute(&db).await.unwrap();
        assert!(!is_online(&db, FRIEND).await.unwrap());
    }

    #[tokio::test]
    async fn ring_then_answer() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        heartbeat(&db, FRIEND).await.unwrap();
        let rung = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        let room = rung["room"].as_str().unwrap().to_string();
        assert!(room.starts_with("call-app-"));
        assert_eq!(rung["callId"], room);
        assert_eq!(rung["state"], "ringing");
        assert_eq!(rung["token"], format!("tok:{room}:user-{OWNER}:true"));
        assert_eq!(rung["url"], "wss://livekit.test");
        let thread = rung["threadId"].as_str().unwrap().to_string();

        // the callee's poll shows it, a contact sees the owner's first name
        let seen = incoming(&db, FRIEND).await.unwrap();
        assert_eq!(seen["calls"][0]["room"], room);
        assert_eq!(seen["calls"][0]["from"]["label"], "Olive");
        // the caller sees nothing incoming, and a second ring finds the callee busy
        assert!(incoming(&db, OWNER).await.unwrap()["calls"].as_array().unwrap().is_empty());
        assert!(matches!(ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await, Err(CallError::Phone(PhoneError::Conflict("callee_busy")))));

        // only the callee can answer, and only for the right caller
        assert!(matches!(answer(&db, &lk, OWNER, FRIEND, &room).await, Err(CallError::Phone(PhoneError::NotFound("call_not_found")))));
        assert!(matches!(answer(&db, &lk, FRIEND, STRANGER, &room).await, Err(CallError::Phone(PhoneError::NotFound("call_not_found")))));
        let answered = answer(&db, &lk, FRIEND, OWNER, &room).await.unwrap();
        assert_eq!(answered["token"], format!("tok:{room}:user-{FRIEND}:true"));
        assert_eq!(answered["threadId"], thread);
        // once answered it's over for everyone else
        assert!(matches!(answer(&db, &lk, FRIEND, OWNER, &room).await, Err(CallError::Gone("call_over"))));
        assert!(matches!(cancel(&db, OWNER, FRIEND, &room).await, Err(CallError::Gone("call_over"))));
        assert!(incoming(&db, FRIEND).await.unwrap()["calls"].as_array().unwrap().is_empty());
        assert_eq!(kinds(&db, &thread).await, ["call.ring", "call.answered"]);
    }

    #[tokio::test]
    async fn contact_can_ring_the_owner_without_naming_the_bot() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        heartbeat(&db, OWNER).await.unwrap();
        let rung = ring(&db, &lk, FRIEND, OWNER, "", "voice").await.unwrap();
        let seen = incoming(&db, OWNER).await.unwrap();
        assert_eq!(seen["calls"][0]["botId"], BOT);
        assert_eq!(seen["calls"][0]["from"]["label"], "Sam", "the owner sees the label they gave the contact");
        assert_eq!(rung["state"], "ringing");
    }

    #[tokio::test]
    async fn decline_and_cancel() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        heartbeat(&db, FRIEND).await.unwrap();
        let first = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        let room = first["room"].as_str().unwrap().to_string();
        let thread = first["threadId"].as_str().unwrap().to_string();
        // the caller can't decline their own ring
        assert!(matches!(decline(&db, OWNER, FRIEND, &room).await, Err(CallError::Phone(PhoneError::NotFound(_)))));
        decline(&db, FRIEND, OWNER, &room).await.unwrap();
        assert!(matches!(decline(&db, FRIEND, OWNER, &room).await, Err(CallError::Gone(_))));

        let second = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        let room2 = second["room"].as_str().unwrap().to_string();
        // the callee can't cancel the caller's ring
        assert!(matches!(cancel(&db, FRIEND, OWNER, &room2).await, Err(CallError::Phone(PhoneError::NotFound(_)))));
        cancel(&db, OWNER, FRIEND, &room2).await.unwrap();
        assert!(matches!(answer(&db, &lk, FRIEND, OWNER, &room2).await, Err(CallError::Gone("call_over"))));
        assert_eq!(kinds(&db, &thread).await, ["call.ring", "call.declined", "call.ring", "call.cancelled"]);
    }

    #[tokio::test]
    async fn unanswered_ring_becomes_a_missed_call() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        heartbeat(&db, FRIEND).await.unwrap();
        let rung = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        let room = rung["room"].as_str().unwrap().to_string();
        let thread = rung["threadId"].as_str().unwrap().to_string();
        sqlx::query("UPDATE inapp_calls SET expires_at = now() - interval '1 second' WHERE room = $1").bind(&room).execute(&db).await.unwrap();
        // answering a ring that ran out fails, and the sweep ran exactly once
        assert!(matches!(answer(&db, &lk, FRIEND, OWNER, &room).await, Err(CallError::Gone("call_over"))));
        assert_eq!(expire_stale(&db).await.unwrap(), 0);
        assert_eq!(kinds(&db, &thread).await, ["call.ring", "call.missed"]);
        let state: String = sqlx::query_scalar("SELECT state FROM inapp_calls WHERE room = $1").bind(&room).fetch_one(&db).await.unwrap();
        assert_eq!(state, "missed");
    }

    #[tokio::test]
    async fn offline_callee_gets_a_409_and_a_missed_call_in_the_thread() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        let err = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap_err();
        let CallError::Offline { thread_id } = err else { panic!("expected callee_offline, got {err:?}") };
        assert_eq!(kinds(&db, &thread_id).await, ["call.missed"]);
        let resp = CallError::Offline { thread_id: thread_id.clone() }.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "callee_offline");
        assert_eq!(body["message"], "We'll let them know you called");
        // the callee reads the missed call from the shared thread
        let events = list_events(&db, FRIEND, &thread_id, 0, 10).await.unwrap();
        assert_eq!(events["events"][0]["kind"], "call.missed");
        assert_eq!(events["events"][0]["senderUserId"], OWNER);
    }

    #[tokio::test]
    async fn auto_answer_dispatches_the_voice_agent_even_when_offline() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        sqlx::query("INSERT INTO voice_bot_config (bot_id, user_id) VALUES ('friend_bot', $1)").bind(FRIEND).execute(&db).await.unwrap();
        // someone else's bot can't be set
        assert!(matches!(set_auto_answer(&db, OWNER, Some("friend_bot")).await, Err(CallError::Phone(PhoneError::Forbidden("not_your_bot")))));
        set_auto_answer(&db, FRIEND, Some("friend_bot")).await.unwrap();
        let rung = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        assert_eq!(rung["state"], "answered");
        assert_eq!(rung["answeredBy"], "agent");
        let rooms = lk.rooms.lock().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].1, "allternit-voice");
        let meta: Value = serde_json::from_str(&rooms[0].2).unwrap();
        assert_eq!(meta["botId"], "friend_bot");
        assert_eq!(meta["ownerId"], FRIEND);
        drop(rooms);
        assert_eq!(kinds(&db, rung["threadId"].as_str().unwrap()).await, ["call.answered"]);
        // turned off again, an offline callee is missed
        set_auto_answer(&db, FRIEND, None).await.unwrap();
        assert!(matches!(ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await, Err(CallError::Offline { .. })));
    }

    #[tokio::test]
    async fn rings_are_rate_limited_and_voice_only() {
        let db = pool().await;
        let lk = FakeLiveKit::default();
        assert!(matches!(ring(&db, &lk, OWNER, FRIEND, BOT, "video").await, Err(CallError::Phone(PhoneError::BadRequest(_)))));
        heartbeat(&db, FRIEND).await.unwrap();
        for _ in 0..MAX_RINGS_PER_10_MIN {
            let r = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
            cancel(&db, OWNER, FRIEND, r["room"].as_str().unwrap()).await.unwrap();
        }
        assert!(matches!(ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await, Err(CallError::Phone(PhoneError::TooMany("too_many_rings")))));
    }

    /// Wait for the background push tasks to hand `n` pushes to the recorder.
    async fn wait_for_pushes(rec: &crate::routes::web_push::tests::Recorder, n: usize) {
        for _ in 0..100 {
            if rec.sent.lock().unwrap().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("expected {n} pushes, saw {}", rec.sent.lock().unwrap().len());
    }

    /// Decrypt the `n`th recorded push the way the friend's browser would.
    fn pushed(rec: &crate::routes::web_push::tests::Recorder, n: usize, key: &aws_lc_rs::agreement::PrivateKey, public: &[u8], auth: &[u8]) -> Value {
        let sent = rec.sent.lock().unwrap();
        serde_json::from_slice(&crate::routes::web_push::tests::decrypt(&sent[n].2, key, public, auth)).unwrap()
    }

    #[tokio::test]
    async fn closed_app_with_a_push_device_rings_then_shows_missed_call_and_messages() {
        let (public, private) = crate::routes::web_push::tests::fixture_keys();
        std::env::set_var("ALLTERNIT_VAPID_PUBLIC_KEY", public);
        std::env::set_var("ALLTERNIT_VAPID_PRIVATE_KEY", private);
        let rec = Arc::new(crate::routes::web_push::tests::Recorder::default());
        *crate::routes::web_push::TEST_TRANSPORT.lock().unwrap() = Some(rec.clone());

        let db = pool().await;
        let lk = FakeLiveKit::default();
        let (p256dh, auth_b64, key, auth) = crate::routes::web_push::tests::browser_sub();
        let ua_public = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &p256dh).unwrap();

        // no device registered: still an instant missed call, nothing pushed
        assert!(matches!(ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await, Err(CallError::Offline { .. })));
        assert!(rec.sent.lock().unwrap().is_empty());

        // the friend registers a device; their app is closed (no presence), yet the ring goes through
        crate::routes::web_push::subscribe(&db, FRIEND, "phone", "https://push.example.com/friend", &p256dh, &auth_b64, "test").await.unwrap();
        let rung = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        assert_eq!(rung["state"], "ringing");
        let room = rung["room"].as_str().unwrap().to_string();
        let thread = rung["threadId"].as_str().unwrap().to_string();
        wait_for_pushes(&rec, 1).await;
        let ring_push = pushed(&rec, 0, &key, &ua_public, &auth);
        assert_eq!(ring_push["type"], "call");
        assert_eq!(ring_push["title"], "Incoming call from Olive");
        assert_eq!(ring_push["url"], format!("/?allternit_call={room}&from={OWNER}&thread={thread}&bot={BOT}"));
        assert_eq!(ring_push["tag"], format!("call-{room}"));

        // the caller hangs up first: the ringing notification is replaced by a missed call
        cancel(&db, OWNER, FRIEND, &room).await.unwrap();
        wait_for_pushes(&rec, 2).await;
        let missed = pushed(&rec, 1, &key, &ua_public, &auth);
        assert_eq!((missed["type"].as_str(), missed["tag"].as_str()), (Some("missed_call"), Some(format!("call-{room}").as_str())));
        assert_eq!(missed["title"], "Missed call from Olive");

        // a ring nobody answers in time also ends as a missed call push
        let again = ring(&db, &lk, OWNER, FRIEND, BOT, "voice").await.unwrap();
        wait_for_pushes(&rec, 3).await;
        sqlx::query("UPDATE inapp_calls SET expires_at = now() - interval '1 second' WHERE room = $1").bind(again["room"].as_str().unwrap()).execute(&db).await.unwrap();
        expire_stale(&db).await.unwrap();
        wait_for_pushes(&rec, 4).await;
        assert_eq!(pushed(&rec, 3, &key, &ua_public, &auth)["type"], "missed_call");

        // a thread message pushes the other person, once per 20 s per thread
        send_message(&db, OWNER, &thread, "are you there?").await.unwrap();
        wait_for_pushes(&rec, 5).await;
        let msg = pushed(&rec, 4, &key, &ua_public, &auth);
        assert_eq!((msg["type"].as_str(), msg["title"].as_str(), msg["body"].as_str()), (Some("message"), Some("Olive"), Some("are you there?")));
        assert_eq!(msg["url"], format!("/?allternit_thread={thread}&bot={BOT}&from={OWNER}"));
        send_message(&db, OWNER, &thread, "hello?").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(rec.sent.lock().unwrap().len(), 5, "second message inside the window is collapsed");
        // the author is never pushed about their own message, and the owner has no device registered
        send_message(&db, FRIEND, &thread, "here!").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(rec.sent.lock().unwrap().len(), 5);
    }
}
