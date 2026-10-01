//! Channel Packs on the gateway tables (spec channel-packs.md, "Mapping and
//! sync contract"): one Thread <-> one `channel_conversation_bindings` row per
//! external conversation, an append-only `channel_message_log` for stable
//! remote ids / correlation ids / dedupe, and `bot_events` carrying
//! `channel.*` events tagged with the thread.
//!
//! Every platform sits behind [`ChannelTransport`] (inbound verify+normalize,
//! outbound post, identity, cursor fetch). Slack is the first implementation;
//! Teams / Discord / WhatsApp live in `channel_transports.rs`.
//!
//! Outbound goes through: binding read-only check -> channel_tools policy
//! (`channel.send` / `<provider>.send`: deny / ask) -> Allternit approval for
//! consequential posts -> transport -> stored remote id as
//! `last_outbound_cursor`. An uncertain delivery (timeout, 5xx) is stored as
//! `unconfirmed` and never re-posted blindly; the inbound echo or a resume
//! confirms it.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_gateway_routes::{id, now};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::gateway_runner::{create_approval, led, Cx};
use crate::AppState;

// ---------------------------------------------------------------- transport contract

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundKind {
    Message,
    ReactionUpdated,
    Edited,
    Deleted,
    /// Delivery receipt for one of our posts (WhatsApp statuses).
    Delivery,
}

impl InboundKind {
    pub fn event_type(self) -> &'static str {
        match self {
            InboundKind::Message => "channel.message.received",
            InboundKind::ReactionUpdated => "channel.reaction.updated",
            InboundKind::Edited => "channel.message.edited",
            InboundKind::Deleted => "channel.message.deleted",
            InboundKind::Delivery => "channel.message.delivery",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            InboundKind::Message => "message",
            InboundKind::ReactionUpdated => "reaction",
            InboundKind::Edited => "edited",
            InboundKind::Deleted => "deleted",
            InboundKind::Delivery => "delivery",
        }
    }
}

/// A normalized inbound platform event.
#[derive(Debug, Clone, PartialEq)]
pub struct Inbound {
    pub kind: InboundKind,
    pub workspace: Option<String>,
    pub channel: String,
    /// Stable external conversation key, e.g. `slack:<channel>:<root ts>`.
    pub conversation: String,
    /// The platform's own thread id inside the conversation (Slack root ts).
    pub thread: Option<String>,
    /// Stable id for dedupe: unique per (message | edit version | reaction change).
    pub remote_id: String,
    /// Id of the message the event is about (reactions/edits/deletes point at one).
    pub message_id: String,
    pub text: Option<String>,
    pub user: Option<String>,
    pub reaction: Option<String>,
    pub added: Option<bool>,
    /// Monotonic per-conversation cursor (Slack ts, Discord snowflake, WA timestamp...).
    pub cursor: Option<String>,
    /// Authored by our own bot/app: an echo of an outbound post, not user input.
    pub own: bool,
}

#[derive(Debug, Clone)]
pub struct Outbound {
    pub workspace: Option<String>,
    pub channel: String,
    pub thread: Option<String>,
    pub text: String,
    pub identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    pub remote_id: String,
    /// Posted by the app on the bot's behalf rather than as the exact identity.
    pub relayed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PostError {
    /// The platform said no (definite: nothing was posted).
    Rejected(String),
    /// Timeout / transport failure / 5xx: the message may or may not exist.
    Uncertain(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: Option<String>,
    /// The platform lets us post as exactly this identity.
    pub exact: bool,
}

#[async_trait]
pub trait ChannelTransport: Send + Sync {
    fn provider(&self) -> &'static str;
    /// Verify the platform signature/token on an inbound webhook.
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String>;
    /// Payload -> zero or more normalized events (unknown shapes yield none).
    fn normalize(&self, payload: &Value) -> Vec<Inbound>;
    fn identity(&self, requested: Option<&str>) -> Identity;
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError>;
    /// Events after `cursor` for a conversation (reconnect). Default: none.
    async fn fetch_since(&self, _channel: &str, _thread: Option<&str>, _cursor: Option<&str>) -> Result<Vec<Inbound>, String> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------- bindings

#[derive(Debug, Clone)]
pub struct BindingRow {
    pub id: String,
    pub owner: String,
    pub thread_id: String,
    pub provider: String,
    pub channel: Option<String>,
    pub workspace: Option<String>,
    pub conversation: String,
    pub external_thread: Option<String>,
    pub last_inbound: Option<String>,
    pub read_only: bool,
    pub bidirectional: bool,
    pub posting_identity: Option<String>,
    pub account: Option<String>,
}

const B_COLS: &str = "id, owner, thread_id, provider, external_channel_id, external_workspace_id, external_conversation_id, external_thread_id, last_inbound_cursor, read_only, bidirectional, posting_identity_id, account_binding_id";

fn b_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BindingRow> {
    Ok(BindingRow {
        id: r.get(0)?,
        owner: r.get(1)?,
        thread_id: r.get(2)?,
        provider: r.get(3)?,
        channel: r.get(4)?,
        workspace: r.get(5)?,
        conversation: r.get(6)?,
        external_thread: r.get(7)?,
        last_inbound: r.get(8)?,
        read_only: r.get::<_, i64>(9)? != 0,
        bidirectional: r.get::<_, i64>(10)? != 0,
        posting_identity: r.get(11)?,
        account: r.get(12)?,
    })
}

pub fn find_binding(db: &DbHandle, provider: &str, conversation: &str) -> Option<BindingRow> {
    let conn = db.connect().ok()?;
    conn.query_row(
        &format!("SELECT {B_COLS} FROM channel_conversation_bindings WHERE provider = ?1 AND external_conversation_id = ?2 ORDER BY created_at LIMIT 1"),
        params![provider, conversation],
        b_row,
    )
    .ok()
}

pub fn binding_for_thread(db: &DbHandle, owner: &str, thread_id: &str) -> Option<BindingRow> {
    let conn = db.connect().ok()?;
    conn.query_row(
        &format!("SELECT {B_COLS} FROM channel_conversation_bindings WHERE owner = ?1 AND thread_id = ?2 ORDER BY created_at DESC LIMIT 1"),
        params![owner, thread_id],
        b_row,
    )
    .ok()
}

/// Find the binding a message/reaction belongs to inside a channel: the
/// conversation key of any of `message_ids`' roots, or a logged message id.
pub fn find_binding_by_message(db: &DbHandle, provider: &str, channel: &str, message_id: &str) -> Option<BindingRow> {
    let conn = db.connect().ok()?;
    conn.query_row(
        &format!(
            "SELECT {} FROM channel_conversation_bindings b WHERE b.provider = ?1 AND b.external_channel_id = ?2 AND
               (b.external_thread_id = ?3 OR EXISTS (SELECT 1 FROM channel_message_log l WHERE l.binding_id = b.id AND l.remote_id LIKE ?4))
             ORDER BY b.created_at LIMIT 1",
            B_COLS.split(", ").map(|c| format!("b.{c}")).collect::<Vec<_>>().join(", ")
        ),
        params![provider, channel, message_id, format!("%{message_id}%")],
        b_row,
    )
    .ok()
}

pub fn ensure_binding(db: &DbHandle, owner: &str, thread_id: &str, provider: &str, ev: &Inbound) -> rusqlite::Result<BindingRow> {
    if let Some(b) = find_binding(db, provider, &ev.conversation) {
        return Ok(b);
    }
    let conn = db.connect()?;
    let account: Option<String> = conn
        .query_row("SELECT id FROM provider_account_bindings WHERE owner = ?1 AND vendor = ?2 ORDER BY created_at LIMIT 1", params![owner, provider], |r| r.get(0))
        .optional()?;
    let bid = id("ccb");
    conn.execute(
        "INSERT INTO channel_conversation_bindings (id, owner, thread_id, provider, account_binding_id, external_workspace_id, external_channel_id,
            external_conversation_id, external_thread_id, bidirectional, read_only, sync_state, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,0,'LIVE',?10,?10)",
        params![bid, owner, thread_id, provider, account, ev.workspace, ev.channel, ev.conversation, ev.thread, now()],
    )?;
    find_binding(db, provider, &ev.conversation).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

fn ts_key(s: &str) -> (u64, u64, String) {
    let (a, b) = s.split_once('.').unwrap_or((s, ""));
    (a.parse().unwrap_or(0), b.parse().unwrap_or(0), s.to_string())
}

/// True when cursor `a` is strictly after `b` (numeric-aware; falls back to text).
pub fn cursor_after(a: &str, b: &str) -> bool {
    match (a.parse::<u128>(), b.parse::<u128>()) {
        (Ok(x), Ok(y)) => x > y,
        _ => ts_key(a) > ts_key(b),
    }
}

// ---------------------------------------------------------------- inbound

#[derive(Debug, Clone, PartialEq)]
pub enum Recorded {
    New,
    /// Already seen (a replay or a Slack retry): nothing was emitted.
    Duplicate,
    /// Our own message coming back; confirmed a pending/unconfirmed post.
    Echo,
}

fn bot_of_thread(db: &DbHandle, thread_id: &str) -> Option<String> {
    db.connect().ok()?.query_row("SELECT bot_id FROM bot_threads WHERE id = ?1", params![thread_id], |r| r.get(0)).ok()
}

/// Record an inbound event once: stable remote id, correlation id, cursor
/// advance, and the `channel.*` ledger event.
pub fn record_inbound(db: &DbHandle, b: &BindingRow, ev: &Inbound) -> Result<Recorded, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    if ev.own {
        // Our own post coming back: confirm an unconfirmed outbound with the same text.
        let n = conn
            .execute(
                "UPDATE channel_message_log SET state = 'confirmed', remote_id = ?3, updated_at = ?4
                 WHERE id = (SELECT id FROM channel_message_log WHERE binding_id = ?1 AND direction = 'outbound' AND state IN ('pending','unconfirmed')
                             AND json_extract(detail_json, '$.text') = ?2 ORDER BY created_at LIMIT 1)",
                params![b.id, ev.text.clone().unwrap_or_default(), ev.message_id, now()],
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            advance(&conn, &b.id, "last_outbound_cursor", &ev.message_id);
        }
        return Ok(Recorded::Echo);
    }
    let corr = format!("{}:{}", b.provider, ev.remote_id);
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, correlation_id, state, detail_json, created_at, updated_at)
             VALUES (?1,?2,?3,?4,'inbound',?5,?6,?7,'confirmed',?8,?9,?9)",
            params![
                id("cml"),
                b.owner,
                b.id,
                b.thread_id,
                ev.kind.as_str(),
                ev.remote_id,
                corr,
                json!({ "text": ev.text, "user": ev.user, "reaction": ev.reaction, "added": ev.added, "messageId": ev.message_id }).to_string(),
                now()
            ],
        )
        .map_err(|e| e.to_string())?;
    if inserted == 0 {
        return Ok(Recorded::Duplicate);
    }
    // Delivery contract (`channel.message.delivery`): `messageId` is the platform id of
    // our outbound post, `state` the outbound log state it moved to (confirmed|failed),
    // `delivery` the raw platform status (sent|delivered|read|failed).
    let delivery_state = (ev.kind == InboundKind::Delivery).then(|| if ev.text.as_deref() == Some("failed") { "failed" } else { "confirmed" });
    if let Some(to) = delivery_state {
        let status = ev.text.clone().unwrap_or_default();
        let _ = conn.execute(
            "UPDATE channel_message_log SET state = ?1, updated_at = ?2, detail_json = json_set(detail_json, '$.delivery', ?3)
             WHERE binding_id = ?4 AND direction = 'outbound' AND remote_id = ?5",
            params![to, now(), status, b.id, ev.message_id],
        );
    }
    if ev.kind == InboundKind::Message {
        if let Some(c) = &ev.cursor {
            if b.last_inbound.as_deref().map_or(true, |l| cursor_after(c, l)) {
                advance(&conn, &b.id, "last_inbound_cursor", c);
            }
        }
    }
    if let Some(bot) = bot_of_thread(db, &b.thread_id) {
        led(
            db,
            &bot,
            &b.thread_id,
            None,
            ev.kind.event_type(),
            ("user", ev.user.as_deref().unwrap_or("unknown")),
            {
                let mut p = json!({
                    "provider": b.provider, "bindingId": b.id, "conversation": b.conversation, "remoteId": ev.remote_id,
                    "messageId": ev.message_id, "correlationId": corr, "text": ev.text, "reaction": ev.reaction, "added": ev.added,
                });
                if let Some(to) = delivery_state {
                    p["state"] = json!(to);
                    p["delivery"] = json!(ev.text);
                }
                p
            },
            Some(format!("chan:{}:in:{}", b.id, ev.remote_id)),
        );
    }
    Ok(Recorded::New)
}

fn advance(conn: &rusqlite::Connection, binding: &str, col: &str, value: &str) {
    let _ = conn.execute(&format!("UPDATE channel_conversation_bindings SET {col} = ?1, updated_at = ?2 WHERE id = ?3"), params![value, now(), binding]);
}

/// Reconnect: pull everything after `lastInboundCursor` and record it. Replays
/// of what we already have are no-ops. Returns the number of new events.
pub async fn resume(db: &DbHandle, tx: &dyn ChannelTransport, b: &BindingRow) -> Result<usize, String> {
    let fresh = find_binding(db, &b.provider, &b.conversation).unwrap_or_else(|| b.clone());
    let mut events = tx.fetch_since(fresh.channel.as_deref().unwrap_or_default(), fresh.external_thread.as_deref(), fresh.last_inbound.as_deref()).await?;
    events.sort_by(|a, b| match (&a.cursor, &b.cursor) {
        (Some(x), Some(y)) if cursor_after(x, y) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) if cursor_after(y, x) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    });
    let mut n = 0;
    for ev in events {
        let cur = find_binding(db, &b.provider, &b.conversation).unwrap_or_else(|| b.clone());
        if record_inbound(db, &cur, &ev)? == Recorded::New {
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------- outbound

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendReq {
    pub text: String,
    pub posting_identity_id: Option<String>,
    pub correlation_id: Option<String>,
    pub consequential: Option<bool>,
    pub allternit_approval_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SendOutcome {
    Sent { remote_id: String, relayed: bool, correlation_id: String },
    /// Delivery uncertain: shown as Pending/Unconfirmed, not re-posted.
    Unconfirmed { correlation_id: String },
    ApprovalRequired { approval_id: String },
    Denied(String),
    ReadOnly,
    NoBinding,
    Rejected(String),
    /// Same correlation id as an earlier send; that send's state.
    Replay { state: String, remote_id: Option<String> },
}

/// `Some("deny" | "ask")` for this bot's `channel.send` rule on the provider.
fn send_policy(db: &DbHandle, bot_id: &str, provider: &str) -> Option<String> {
    let rules = crate::channel_tools::channel_rules(db, bot_id, provider)?;
    let mut ask = false;
    for r in rules.as_array()? {
        let perm = r["permission"].as_str().unwrap_or_default();
        if perm == "channel.send" || perm == format!("{provider}.send") {
            match r["action"].as_str() {
                Some("deny") => return Some("deny".into()),
                Some("ask") => ask = true,
                _ => {}
            }
        }
    }
    ask.then(|| "ask".to_string())
}

/// Every outbound attempt that does not post is still on the record: one
/// `channel_message_log` row in `state` and one `channel.message.sent` event whose
/// `delivery` tells the transcript it did not go out (never a silent drop).
#[allow(clippy::too_many_arguments)]
fn log_unposted(conn: &rusqlite::Connection, db: &DbHandle, owner: &str, b: &BindingRow, bot_id: &str, thread_id: &str, corr: &str, text: &str, state: &str, delivery: &str, reason: &str) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, correlation_id, state, detail_json, created_at, updated_at)
         VALUES (?1,?2,?3,?4,'outbound','message',NULL,?5,?6,?7,?8,?8)",
        params![id("cml"), owner, b.id, thread_id, corr, state, json!({ "text": text, "reason": reason }).to_string(), now()],
    );
    led(
        db,
        bot_id,
        thread_id,
        None,
        "channel.message.sent",
        ("bot", bot_id),
        json!({ "provider": b.provider, "bindingId": b.id, "correlationId": corr, "text": text, "state": state, "delivery": delivery, "reason": reason }),
        Some(format!("chan:{}:{state}:{corr}", b.id)),
    );
}

pub async fn send(db: &DbHandle, tx: &dyn ChannelTransport, owner: &str, thread_id: &str, req: &SendReq) -> Result<SendOutcome, String> {
    let (bot_id, generation, session_id): (String, i64, String) = {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let Some(bot) = conn
            .query_row("SELECT bot_id FROM bot_threads WHERE id = ?1 AND user_id = ?2", params![thread_id, owner], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
        else {
            return Err("thread not found".into());
        };
        let (g, sid) = conn
            .query_row("SELECT generation, session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1", params![thread_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((1, String::new()));
        (bot, g, sid)
    };
    let Some(b) = binding_for_thread(db, owner, thread_id).filter(|b| b.provider == tx.provider()) else { return Ok(SendOutcome::NoBinding) };
    if b.read_only || !b.bidirectional {
        return Ok(SendOutcome::ReadOnly);
    }
    let text = req.text.trim();
    if text.is_empty() {
        return Err("text is required".into());
    }
    let corr = req.correlation_id.clone().unwrap_or_else(|| id("out"));
    let conn = db.connect().map_err(|e| e.to_string())?;
    // Replay of an earlier send: report it, never post twice.
    let prior: Option<(String, Option<String>)> = conn
        .query_row("SELECT state, remote_id FROM channel_message_log WHERE binding_id = ?1 AND direction = 'outbound' AND correlation_id = ?2", params![b.id, corr], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((state, remote_id)) = prior {
        // Held or refused sends are re-evaluated (policy may have changed, approval may have landed).
        if state != "awaiting_approval" && state != "denied" {
            return Ok(SendOutcome::Replay { state, remote_id });
        }
    }

    // Policy, then Allternit approval for consequential posts.
    let policy = send_policy(db, &bot_id, &b.provider);
    if policy.as_deref() == Some("deny") {
        let why = format!("this bot may not post to {} (channel policy)", b.provider);
        log_unposted(&conn, db, owner, &b, &bot_id, thread_id, &corr, text, "denied", "failed", &why);
        return Ok(SendOutcome::Denied(why));
    }
    let needs_approval = policy.as_deref() == Some("ask") || req.consequential.unwrap_or(false);
    let mut consume: Option<String> = None;
    if needs_approval {
        let ok = match &req.allternit_approval_id {
            Some(a) => conn
                .query_row(
                    "SELECT COUNT(*) FROM gateway_approvals WHERE id = ?1 AND owner = ?2 AND thread_id = ?3 AND authority = 'allternit' AND state = 'approved' AND consumed = 0",
                    params![a, owner, thread_id],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(|e| e.to_string())?
                > 0,
            None => false,
        };
        if !ok {
            let pending: Option<String> = conn
                .query_row(
                    "SELECT id FROM gateway_approvals WHERE owner = ?1 AND thread_id = ?2 AND authority = 'allternit' AND state = 'pending' AND correlation_id = ?3",
                    params![owner, thread_id, corr],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let aid = match pending {
                Some(a) => a,
                None => {
                    let cx = Cx { owner: owner.into(), thread_id: thread_id.into(), bot_id: bot_id.clone(), generation, session_id, exec: json!({ "vendor": b.provider }) };
                    create_approval(db, &cx, "allternit", &format!("post to {}", b.provider), None, json!({ "text": text, "channel": b.channel }), Some(&corr)).map_err(|e| e.message)?
                }
            };
            log_unposted(&conn, db, owner, &b, &bot_id, thread_id, &corr, text, "awaiting_approval", "pending", &format!("waiting for approval {aid}"));
            return Ok(SendOutcome::ApprovalRequired { approval_id: aid });
        }
        consume = req.allternit_approval_id.clone();
    }

    let identity = tx.identity(req.posting_identity_id.as_deref().or(b.posting_identity.as_deref()));
    conn.execute(
        "INSERT OR REPLACE INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, correlation_id, state, detail_json, created_at, updated_at)
         VALUES (?1,?2,?3,?4,'outbound','message',NULL,?5,'pending',?6,?7,?7)",
        params![id("cml"), owner, b.id, thread_id, corr, json!({ "text": text, "identity": identity.id }).to_string(), now()],
    )
    .map_err(|e| e.to_string())?;
    let out = Outbound { workspace: b.workspace.clone(), channel: b.channel.clone().unwrap_or_default(), thread: b.external_thread.clone(), text: text.to_string(), identity: identity.id.clone() };
    let result = tx.post(&out).await;
    let set = |state: &str, remote: Option<&str>| {
        let _ = conn.execute(
            "UPDATE channel_message_log SET state = ?1, remote_id = COALESCE(?2, remote_id), updated_at = ?3 WHERE binding_id = ?4 AND direction = 'outbound' AND correlation_id = ?5",
            params![state, remote, now(), b.id, corr],
        );
    };
    match result {
        Ok(r) => {
            set("confirmed", Some(&r.remote_id));
            advance(&conn, &b.id, "last_outbound_cursor", &r.remote_id);
            if let Some(a) = consume {
                let _ = conn.execute("UPDATE gateway_approvals SET consumed = 1 WHERE id = ?1", params![a]);
            }
            led(
                db,
                &bot_id,
                thread_id,
                None,
                "channel.message.sent",
                ("bot", &bot_id),
                json!({ "provider": b.provider, "bindingId": b.id, "remoteId": r.remote_id, "correlationId": corr, "text": text, "state": "confirmed", "delivery": "sent", "relayed": r.relayed, "postingIdentityId": identity.id }),
                Some(format!("chan:{}:out:{corr}", b.id)),
            );
            Ok(SendOutcome::Sent { remote_id: r.remote_id, relayed: r.relayed, correlation_id: corr })
        }
        Err(PostError::Rejected(why)) => {
            set("failed", None);
            led(
                db,
                &bot_id,
                thread_id,
                None,
                "channel.message.sent",
                ("bot", &bot_id),
                json!({ "provider": b.provider, "bindingId": b.id, "correlationId": corr, "text": text, "state": "failed", "delivery": "failed", "reason": why }),
                Some(format!("chan:{}:failed:{corr}", b.id)),
            );
            Ok(SendOutcome::Rejected(why))
        }
        Err(PostError::Uncertain(why)) => {
            set("unconfirmed", None);
            led(
                db,
                &bot_id,
                thread_id,
                None,
                "channel.message.pending",
                ("bot", &bot_id),
                json!({ "provider": b.provider, "bindingId": b.id, "correlationId": corr, "text": text, "state": "unconfirmed", "reason": why }),
                Some(format!("chan:{}:pending:{corr}", b.id)),
            );
            Ok(SendOutcome::Unconfirmed { correlation_id: corr })
        }
    }
}

// ---------------------------------------------------------------- HTTP

pub fn channel_gateway_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/gateway/threads/:thread_id/channel-send", post(channel_send_h))
        .route("/gateway/channel-accounts/:id/telegram-webhook", post(telegram_webhook_h))
}

#[derive(Debug, Deserialize)]
struct WebhookBody {
    url: String,
}

/// Settings → Channels: after cloud-api issues this connection's public
/// address, point the Telegram bot at it (the person's own connection only).
async fn telegram_webhook_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<WebhookBody>) -> Response {
    let Some(acct) = crate::channel_transports::accounts(&state.db, "telegram", Some(&id)).into_iter().find(|a| a.owner == user.user_id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "Telegram connection not found" }))).into_response();
    };
    let token = crate::channel_transports::pick(&acct.secret, "botToken");
    let secret = crate::channel_transports::pick(&acct.secret, "webhookSecret");
    match crate::channel_transports::telegram_set_webhook(&crate::channel_transports::ReqwestSend, &token, &b.url, &secret).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": error }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    text: String,
    #[serde(alias = "postingIdentityId")]
    posting_identity_id: Option<String>,
    correlation_id: Option<String>,
    consequential: Option<bool>,
    allternit_approval_id: Option<String>,
}

/// The transport for a thread's binding (production: real platform clients).
fn transport_for(state: &Arc<AppState>, b: &BindingRow) -> Option<Arc<dyn ChannelTransport>> {
    crate::channel_transports::transport_for(state, b)
}

async fn channel_send_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(thread_id): Path<String>, Json(b): Json<SendBody>) -> Response {
    let Some(binding) = binding_for_thread(&state.db, &user.user_id, &thread_id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this thread has no channel binding", "code": "NO_BINDING" }))).into_response();
    };
    let Some(tx) = transport_for(&state, &binding) else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": format!("{} is not configured", binding.provider), "code": "CHANNEL_OFFLINE" }))).into_response();
    };
    let req = SendReq { text: b.text, posting_identity_id: b.posting_identity_id, correlation_id: b.correlation_id, consequential: b.consequential, allternit_approval_id: b.allternit_approval_id };
    outcome_response(send(&state.db, tx.as_ref(), &user.user_id, &thread_id, &req).await)
}

fn outcome_response(r: Result<SendOutcome, String>) -> Response {
    match r {
        Ok(SendOutcome::Sent { remote_id, relayed, correlation_id }) => Json(json!({ "state": "sent", "remoteId": remote_id, "relayed": relayed, "correlationId": correlation_id })).into_response(),
        Ok(SendOutcome::Unconfirmed { correlation_id }) => (StatusCode::ACCEPTED, Json(json!({ "state": "unconfirmed", "label": "Pending", "correlationId": correlation_id }))).into_response(),
        Ok(SendOutcome::ApprovalRequired { approval_id }) => (StatusCode::PRECONDITION_REQUIRED, Json(json!({ "error": "this post needs your approval first", "code": "APPROVAL_REQUIRED", "approvalId": approval_id }))).into_response(),
        Ok(SendOutcome::Denied(m)) => (StatusCode::FORBIDDEN, Json(json!({ "error": m, "code": "CHANNEL_POLICY" }))).into_response(),
        Ok(SendOutcome::ReadOnly) => (StatusCode::FORBIDDEN, Json(json!({ "error": "this conversation is read-only", "code": "READ_ONLY" }))).into_response(),
        Ok(SendOutcome::NoBinding) => (StatusCode::NOT_FOUND, Json(json!({ "error": "this thread has no channel binding", "code": "NO_BINDING" }))).into_response(),
        Ok(SendOutcome::Rejected(m)) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": m, "code": "CHANNEL_REJECTED" }))).into_response(),
        Ok(SendOutcome::Replay { state, remote_id }) => Json(json!({ "state": state, "remoteId": remote_id, "replay": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

// ---------------------------------------------------------------- Slack

pub struct SlackTransport {
    pub token: Option<String>,
    /// The Slack bot user id the app posts as.
    pub own_identity: Option<String>,
}

impl SlackTransport {
    pub fn from_env() -> Self {
        let cfg = crate::config::AppConfig::load();
        SlackTransport { token: cfg.slack_bot_token(), own_identity: std::env::var("ALLTERNIT_SLACK_BOT_USER_ID").ok().filter(|s| !s.is_empty()) }
    }
}

fn slack_one(ev: &Value, own: bool) -> Option<Inbound> {
    let team = ev.get("team").and_then(Value::as_str).map(str::to_string);
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match ev.get("type").and_then(Value::as_str)? {
        "message" => {
            let channel = s(ev, "channel")?;
            match ev.get("subtype").and_then(Value::as_str) {
                None | Some("bot_message") | Some("thread_broadcast") => {
                    let ts = s(ev, "ts")?;
                    let root = s(ev, "thread_ts").unwrap_or_else(|| ts.clone());
                    Some(Inbound {
                        kind: InboundKind::Message, workspace: team, conversation: format!("slack:{channel}:{root}"), channel, thread: Some(root),
                        remote_id: ts.clone(), message_id: ts.clone(), text: s(ev, "text"), user: s(ev, "user").or_else(|| s(ev, "bot_id")),
                        reaction: None, added: None, cursor: Some(ts), own: own || ev.get("bot_id").is_some(),
                    })
                }
                Some("message_changed") => {
                    let m = ev.get("message")?;
                    let ts = s(m, "ts")?;
                    let root = s(m, "thread_ts").unwrap_or_else(|| ts.clone());
                    let version = m.pointer("/edited/ts").and_then(Value::as_str).unwrap_or(&ts).to_string();
                    Some(Inbound {
                        kind: InboundKind::Edited, workspace: team, conversation: format!("slack:{channel}:{root}"), channel, thread: Some(root),
                        remote_id: format!("edit:{ts}:{version}"), message_id: ts, text: s(m, "text"), user: s(m, "user"), reaction: None, added: None,
                        cursor: None, own: false,
                    })
                }
                Some("message_deleted") => {
                    let ts = s(ev, "deleted_ts")?;
                    let root = ev.pointer("/previous_message/thread_ts").and_then(Value::as_str).unwrap_or(&ts).to_string();
                    Some(Inbound {
                        kind: InboundKind::Deleted, workspace: team, conversation: format!("slack:{channel}:{root}"), channel, thread: Some(root),
                        remote_id: format!("del:{ts}"), message_id: ts, text: None, user: None, reaction: None, added: None, cursor: None, own: false,
                    })
                }
                _ => None,
            }
        }
        t @ ("reaction_added" | "reaction_removed") => {
            let item = ev.get("item")?;
            let channel = s(item, "channel")?;
            let ts = s(item, "ts")?;
            let reaction = s(ev, "reaction")?;
            let user = s(ev, "user");
            let event_ts = s(ev, "event_ts").unwrap_or_default();
            // Reactions don't say which thread; the caller resolves the binding by message id.
            Some(Inbound {
                kind: InboundKind::ReactionUpdated, workspace: team, conversation: format!("slack:{channel}:{ts}"), channel, thread: None,
                remote_id: format!("react:{ts}:{}:{reaction}:{t}:{event_ts}", user.clone().unwrap_or_default()), message_id: ts, text: None, user,
                reaction: Some(reaction), added: Some(t == "reaction_added"), cursor: None, own: false,
            })
        }
        _ => None,
    }
}

#[async_trait]
impl ChannelTransport for SlackTransport {
    fn provider(&self) -> &'static str {
        "slack"
    }
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        crate::slack_webhook_routes::verify_slack_signature(secret, headers, body)
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        let ev = payload.get("event").unwrap_or(payload);
        let own = self.own_identity.as_deref().is_some_and(|me| ev.get("user").and_then(Value::as_str) == Some(me));
        slack_one(ev, own).into_iter().collect()
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        // The Slack app only ever posts as itself.
        let exact = match (requested, self.own_identity.as_deref()) {
            (None, _) => true,
            (Some(r), Some(me)) => r == me,
            (Some(_), None) => false,
        };
        Identity { id: requested.map(str::to_string), exact }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let token = self.token.clone().ok_or_else(|| PostError::Rejected("ALLTERNIT_SLACK_BOT_TOKEN is not configured".into()))?;
        let relayed = !self.identity(out.identity.as_deref()).exact;
        let text = match (&out.identity, relayed) {
            (Some(who), true) => format!("*{who}:* {}", out.text),
            _ => out.text.clone(),
        };
        let mut body = json!({ "channel": out.channel, "text": text });
        if let Some(t) = &out.thread {
            body["thread_ts"] = json!(t);
        }
        let resp = reqwest::Client::new()
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(15))
            .json(&body)
            .send()
            .await
            .map_err(|e| PostError::Uncertain(format!("chat.postMessage request failed: {e}")))?;
        if resp.status().is_server_error() {
            return Err(PostError::Uncertain(format!("chat.postMessage returned {}", resp.status())));
        }
        let v: Value = resp.json().await.map_err(|e| PostError::Uncertain(format!("chat.postMessage response unreadable: {e}")))?;
        match (v.get("ok").and_then(Value::as_bool), v.get("ts").and_then(Value::as_str)) {
            (Some(true), Some(ts)) => Ok(Receipt { remote_id: ts.to_string(), relayed }),
            _ => Err(PostError::Rejected(format!("chat.postMessage failed: {}", v.get("error").and_then(Value::as_str).unwrap_or("unknown")))),
        }
    }
    async fn fetch_since(&self, channel: &str, thread: Option<&str>, cursor: Option<&str>) -> Result<Vec<Inbound>, String> {
        let token = self.token.clone().ok_or("ALLTERNIT_SLACK_BOT_TOKEN is not configured")?;
        let ts = thread.ok_or("no thread to resume")?;
        let mut q = vec![("channel", channel.to_string()), ("ts", ts.to_string())];
        if let Some(c) = cursor {
            q.push(("oldest", c.to_string()));
        }
        let v: Value = reqwest::Client::new()
            .get("https://slack.com/api/conversations.replies")
            .bearer_auth(token)
            .query(&q)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(format!("conversations.replies failed: {}", v.get("error").and_then(Value::as_str).unwrap_or("unknown")));
        }
        Ok(v.get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let mut m = m.clone();
                m["type"] = json!("message");
                m["channel"] = json!(channel);
                slack_one(&m, false)
            })
            .collect())
    }
}

/// Slack Events API entry: edits, deletes, reactions and our own echoes.
/// (Plain user messages take the turn path in `slack_webhook_routes`.)
pub fn ingest_slack_side_event(db: &DbHandle, tx: &dyn ChannelTransport, event: &Value) -> Result<Recorded, String> {
    let Some(ev) = tx.normalize(event).into_iter().next() else { return Ok(Recorded::Duplicate) };
    let binding = match ev.kind {
        InboundKind::ReactionUpdated => find_binding_by_message(db, "slack", &ev.channel, &ev.message_id),
        _ => find_binding(db, "slack", &ev.conversation).or_else(|| find_binding_by_message(db, "slack", &ev.channel, &ev.message_id)),
    };
    let Some(b) = binding else { return Ok(Recorded::Duplicate) };
    record_inbound(db, &b, &ev)
}

/// The bound-bot turn path: find or create the binding for this Slack thread
/// and record the inbound message. `None` = the session has no thread (fall
/// back to the plain path).
pub fn bind_slack_turn(db: &DbHandle, tx: &dyn ChannelTransport, session_id: &str, event: &Value) -> Result<Option<(BindingRow, Recorded)>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let row: Option<(String, String)> = conn
        .query_row("SELECT t.id, t.user_id FROM bot_thread_sessions s JOIN bot_threads t ON t.id = s.thread_id WHERE s.session_id = ?1", params![session_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((thread_id, owner)) = row else { return Ok(None) };
    let Some(ev) = tx.normalize(event).into_iter().next().filter(|e| e.kind == InboundKind::Message) else { return Ok(None) };
    let b = ensure_binding(db, &owner, &thread_id, tx.provider(), &ev).map_err(|e| e.to_string())?;
    let rec = record_inbound(db, &b, &ev)?;
    Ok(Some((b, rec)))
}

/// Post a bot reply into the bound Slack thread and record it as an outbound
/// (idempotent per inbound message: correlation `reply:<thread ts>:<n>`).
pub async fn post_reply(db: &DbHandle, tx: &dyn ChannelTransport, b: &BindingRow, thread_ts: &str, reply: &str) -> Result<(), String> {
    let out = Outbound { workspace: b.workspace.clone(), channel: b.channel.clone().unwrap_or_default(), thread: Some(thread_ts.to_string()), text: reply.to_string(), identity: None };
    let corr = id("reply");
    let conn = db.connect().map_err(|e| e.to_string())?;
    let _ = conn.execute(
        "INSERT INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, correlation_id, state, detail_json, created_at, updated_at)
         VALUES (?1,?2,?3,?4,'outbound','message',NULL,?5,'pending',?6,?7,?7)",
        params![id("cml"), b.owner, b.id, b.thread_id, corr, json!({ "text": reply }).to_string(), now()],
    );
    match tx.post(&out).await {
        Ok(r) => {
            let _ = conn.execute("UPDATE channel_message_log SET state='confirmed', remote_id=?1, updated_at=?2 WHERE binding_id=?3 AND direction='outbound' AND correlation_id=?4", params![r.remote_id, now(), b.id, corr]);
            advance(&conn, &b.id, "last_outbound_cursor", &r.remote_id);
            if let Some(bot) = bot_of_thread(db, &b.thread_id) {
                led(db, &bot, &b.thread_id, None, "channel.message.sent", ("bot", &bot), json!({ "provider": b.provider, "bindingId": b.id, "remoteId": r.remote_id, "correlationId": corr, "text": reply, "state": "confirmed", "delivery": "sent", "relayed": r.relayed }), Some(format!("chan:{}:out:{corr}", b.id)));
            }
            Ok(())
        }
        Err(PostError::Uncertain(why)) => {
            let _ = conn.execute("UPDATE channel_message_log SET state='unconfirmed', updated_at=?1 WHERE binding_id=?2 AND direction='outbound' AND correlation_id=?3", params![now(), b.id, corr]);
            if let Some(bot) = bot_of_thread(db, &b.thread_id) {
                led(db, &bot, &b.thread_id, None, "channel.message.pending", ("bot", &bot), json!({ "provider": b.provider, "bindingId": b.id, "correlationId": corr, "text": reply, "state": "unconfirmed", "reason": why }), Some(format!("chan:{}:pending:{corr}", b.id)));
            }
            Err(format!("reply delivery unconfirmed: {why}"))
        }
        Err(PostError::Rejected(why)) => {
            let _ = conn.execute("UPDATE channel_message_log SET state='failed', updated_at=?1 WHERE binding_id=?2 AND direction='outbound' AND correlation_id=?3", params![now(), b.id, corr]);
            if let Some(bot) = bot_of_thread(db, &b.thread_id) {
                led(db, &bot, &b.thread_id, None, "channel.message.sent", ("bot", &bot), json!({ "provider": b.provider, "bindingId": b.id, "correlationId": corr, "text": reply, "state": "failed", "delivery": "failed", "reason": why }), Some(format!("chan:{}:failed:{corr}", b.id)));
            }
            Err(why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub(crate) struct Fake {
        pub posted: Mutex<Vec<Outbound>>,
        /// "ok" | "reject" | "uncertain"
        pub mode: Mutex<&'static str>,
        pub history: Mutex<Vec<Inbound>>,
        pub own: Option<String>,
    }

    #[async_trait]
    impl ChannelTransport for Fake {
        fn provider(&self) -> &'static str {
            "slack"
        }
        fn verify(&self, _s: &str, _h: &HeaderMap, _b: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn normalize(&self, payload: &Value) -> Vec<Inbound> {
            SlackTransport { token: None, own_identity: self.own.clone() }.normalize(payload)
        }
        fn identity(&self, requested: Option<&str>) -> Identity {
            Identity { id: requested.map(str::to_string), exact: requested.is_none() || requested == self.own.as_deref() }
        }
        async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
            let mode = *self.mode.lock().unwrap();
            self.posted.lock().unwrap().push(out.clone());
            let n = self.posted.lock().unwrap().len();
            match mode {
                "reject" => Err(PostError::Rejected("channel_not_found".into())),
                "uncertain" => Err(PostError::Uncertain("timeout".into())),
                _ => Ok(Receipt { remote_id: format!("1700000100.{n:06}"), relayed: !self.identity(out.identity.as_deref()).exact }),
            }
        }
        async fn fetch_since(&self, _c: &str, _t: Option<&str>, cursor: Option<&str>) -> Result<Vec<Inbound>, String> {
            // A platform replays from `oldest` inclusive.
            Ok(self.history.lock().unwrap().iter().filter(|e| cursor.map_or(true, |c| e.cursor.as_deref().map_or(true, |x| x == c || cursor_after(x, c)))).cloned().collect())
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-chan-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES ('th-1','user-a','bot-1','T','working','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')", []).unwrap();
        c.execute("INSERT INTO bot_thread_sessions (thread_id, generation, session_id, started_at) VALUES ('th-1',1,'s-th-1','2026-01-01T00:00:00Z')", []).unwrap();
        st
    }

    fn msg(ts: &str, thread: Option<&str>, text: &str) -> Value {
        let mut e = json!({ "type": "message", "channel": "C1", "user": "U9", "text": text, "ts": ts, "team": "T1" });
        if let Some(t) = thread {
            e["thread_ts"] = json!(t);
        }
        json!({ "type": "event_callback", "event": e })
    }
    fn ev(f: &Fake, p: &Value) -> Inbound {
        f.normalize(p).remove(0)
    }
    fn events(st: &Arc<AppState>, ty: &str) -> i64 {
        st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id='th-1' AND event_type=?1", params![ty], |r| r.get(0)).unwrap()
    }
    fn bound(st: &Arc<AppState>, f: &Fake) -> BindingRow {
        ensure_binding(&st.db, "user-a", "th-1", "slack", &ev(f, &msg("1700000000.000100", None, "hi"))).unwrap()
    }
    fn set_policy(st: &Arc<AppState>, rules: Value) {
        st.db.connect().unwrap().execute("UPDATE agents SET config = ?1 WHERE id='bot-1'", params![json!({ "channelTools": { "slack": rules } }).to_string()]).unwrap();
    }

    #[tokio::test]
    async fn one_binding_per_slack_thread_dedupes_replays_and_advances_the_cursor() {
        let st = setup("in").await;
        let f = Fake::default();
        let first = ev(&f, &msg("1700000000.000100", None, "hi"));
        assert_eq!(first.conversation, "slack:C1:1700000000.000100");
        let b = ensure_binding(&st.db, "user-a", "th-1", "slack", &first).unwrap();
        // A reply in the same Slack thread finds the same binding.
        let reply = ev(&f, &msg("1700000005.000200", Some("1700000000.000100"), "more"));
        assert_eq!(ensure_binding(&st.db, "user-a", "th-1", "slack", &reply).unwrap().id, b.id);
        assert_eq!(record_inbound(&st.db, &b, &first).unwrap(), Recorded::New);
        assert_eq!(record_inbound(&st.db, &b, &first).unwrap(), Recorded::Duplicate);
        let b2 = find_binding(&st.db, "slack", &b.conversation).unwrap();
        assert_eq!(record_inbound(&st.db, &b2, &reply).unwrap(), Recorded::New);
        // An older message arriving late never moves the cursor back.
        let late = ev(&f, &msg("1700000001.000100", Some("1700000000.000100"), "late"));
        assert_eq!(record_inbound(&st.db, &find_binding(&st.db, "slack", &b.conversation).unwrap(), &late).unwrap(), Recorded::New);
        assert_eq!(find_binding(&st.db, "slack", &b.conversation).unwrap().last_inbound.as_deref(), Some("1700000005.000200"));
        assert_eq!(events(&st, "channel.message.received"), 3);
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM channel_conversation_bindings", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn edits_deletes_and_reactions_become_events_on_the_thread() {
        let st = setup("side").await;
        let f = Fake::default();
        let b = bound(&st, &f);
        let ts = "1700000000.000100";
        record_inbound(&st.db, &b, &ev(&f, &msg(ts, None, "hi"))).unwrap();
        let edited = json!({ "type": "event_callback", "event": { "type": "message", "subtype": "message_changed", "channel": "C1", "message": { "ts": ts, "text": "hi!", "user": "U9", "edited": { "ts": "1700000009.000000" } } } });
        let deleted = json!({ "type": "event_callback", "event": { "type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": ts } });
        let react = json!({ "type": "event_callback", "event": { "type": "reaction_added", "user": "U9", "reaction": "eyes", "item": { "type": "message", "channel": "C1", "ts": ts }, "event_ts": "1700000010.000000" } });
        for p in [&edited, &deleted, &react] {
            assert_eq!(ingest_slack_side_event(&st.db, &f, p).unwrap(), Recorded::New);
            assert_eq!(ingest_slack_side_event(&st.db, &f, p).unwrap(), Recorded::Duplicate);
        }
        assert_eq!(events(&st, "channel.message.edited"), 1);
        assert_eq!(events(&st, "channel.message.deleted"), 1);
        assert_eq!(events(&st, "channel.reaction.updated"), 1);
    }

    #[tokio::test]
    async fn send_posts_to_the_bound_thread_stores_the_cursor_and_flags_relays() {
        let st = setup("send").await;
        let f = Fake { own: Some("UBOT".into()), ..Default::default() };
        let b = bound(&st, &f);
        let r = send(&st.db, &f, "user-a", "th-1", &SendReq { text: "hello".into(), ..Default::default() }).await.unwrap();
        let SendOutcome::Sent { remote_id, relayed, .. } = r else { panic!("{r:?}") };
        assert!(!relayed);
        assert_eq!(f.posted.lock().unwrap()[0].thread.as_deref(), Some("1700000000.000100"));
        assert_eq!(f.posted.lock().unwrap()[0].channel, "C1");
        let cur: Option<String> = st.db.connect().unwrap().query_row("SELECT last_outbound_cursor FROM channel_conversation_bindings WHERE id=?1", params![b.id], |r| r.get(0)).unwrap();
        assert_eq!(cur.as_deref(), Some(remote_id.as_str()));
        let r = send(&st.db, &f, "user-a", "th-1", &SendReq { text: "as gizzi".into(), posting_identity_id: Some("bot-gizzi".into()), ..Default::default() }).await.unwrap();
        assert!(matches!(r, SendOutcome::Sent { relayed: true, .. }));
        assert_eq!(events(&st, "channel.message.sent"), 2);
        let payload: String = st.db.connect().unwrap().query_row("SELECT payload FROM bot_events WHERE event_type='channel.message.sent' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert!(payload.contains("\"relayed\":true"));
        // Another user's thread, or one with no binding, cannot be posted to.
        assert!(send(&st.db, &f, "user-b", "th-1", &SendReq { text: "x".into(), ..Default::default() }).await.is_err());
    }

    #[tokio::test]
    async fn policy_denies_or_asks_and_an_approval_is_single_use() {
        let st = setup("policy").await;
        let f = Fake::default();
        bound(&st, &f);
        set_policy(&st, json!({ "deny": ["channel.send"], "ask": [] }));
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &SendReq { text: "x".into(), ..Default::default() }).await.unwrap(), SendOutcome::Denied(_)));
        set_policy(&st, json!({ "deny": [], "ask": ["channel.send"] }));
        let req = SendReq { text: "ship it".into(), correlation_id: Some("c-1".into()), ..Default::default() };
        let SendOutcome::ApprovalRequired { approval_id } = send(&st.db, &f, "user-a", "th-1", &req).await.unwrap() else { panic!() };
        // The same request reuses its pending approval; nothing was posted.
        assert_eq!(send(&st.db, &f, "user-a", "th-1", &req).await.unwrap(), SendOutcome::ApprovalRequired { approval_id: approval_id.clone() });
        assert!(f.posted.lock().unwrap().is_empty());
        st.db.connect().unwrap().execute("UPDATE gateway_approvals SET state='approved' WHERE id=?1", params![approval_id]).unwrap();
        let ok = SendReq { allternit_approval_id: Some(approval_id.clone()), ..req.clone() };
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &ok).await.unwrap(), SendOutcome::Sent { .. }));
        // Consumed: a different post can't reuse it.
        let again = SendReq { text: "other".into(), correlation_id: Some("c-2".into()), allternit_approval_id: Some(approval_id), ..Default::default() };
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &again).await.unwrap(), SendOutcome::ApprovalRequired { .. }));
        // Explicitly consequential posts need approval even with no rule.
        set_policy(&st, json!({}));
        let c = SendReq { text: "wire it".into(), consequential: Some(true), ..Default::default() };
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &c).await.unwrap(), SendOutcome::ApprovalRequired { .. }));
    }

    /// Handoff step 1b: every outbound outcome — sent, denied, approval-pending, rejected,
    /// unconfirmed, and the Slack reply path — leaves one log row and one ledger event.
    #[tokio::test]
    async fn every_outbound_outcome_writes_a_log_row_and_a_ledger_event() {
        let st = setup("outlog").await;
        let f = Fake::default();
        let b = bound(&st, &f);
        let row = |corr: &str| -> (String, i64) {
            st.db.connect().unwrap()
                .query_row("SELECT state, COUNT(*) FROM channel_message_log WHERE direction='outbound' AND correlation_id=?1", params![corr], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        let ev_for = |corr: &str| -> Vec<(String, Value)> {
            let c = st.db.connect().unwrap();
            let mut q = c.prepare("SELECT event_type, payload FROM bot_events WHERE thread_id='th-1' AND json_extract(payload,'$.correlationId')=?1 ORDER BY seq").unwrap();
            q.query_map(params![corr], |r| Ok((r.get::<_, String>(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap()))).unwrap().map(Result::unwrap).collect()
        };
        let req = |c: &str| SendReq { text: format!("t-{c}"), correlation_id: Some(c.into()), ..Default::default() };

        // sent
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("ok")).await.unwrap(), SendOutcome::Sent { .. }));
        assert_eq!(row("ok"), ("confirmed".into(), 1));
        let e = ev_for("ok");
        assert_eq!((e.len(), e[0].0.as_str(), e[0].1["delivery"].as_str()), (1, "channel.message.sent", Some("sent")));

        // denied: nothing posted, still on the record; a later retry is re-evaluated, not replayed
        set_policy(&st, json!({ "deny": ["channel.send"], "ask": [] }));
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("den")).await.unwrap(), SendOutcome::Denied(_)));
        assert_eq!(row("den"), ("denied".into(), 1));
        let e = ev_for("den");
        assert_eq!((e[0].0.as_str(), e[0].1["state"].as_str(), e[0].1["delivery"].as_str()), ("channel.message.sent", Some("denied"), Some("failed")));
        set_policy(&st, json!({}));
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("den")).await.unwrap(), SendOutcome::Sent { .. }));
        assert_eq!(row("den"), ("confirmed".into(), 1));

        // approval-pending, asked twice: one row, one event
        set_policy(&st, json!({ "deny": [], "ask": ["channel.send"] }));
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("ask")).await.unwrap(), SendOutcome::ApprovalRequired { .. }));
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("ask")).await.unwrap(), SendOutcome::ApprovalRequired { .. }));
        assert_eq!(row("ask"), ("awaiting_approval".into(), 1));
        let e = ev_for("ask");
        assert_eq!((e.len(), e[0].1["state"].as_str(), e[0].1["delivery"].as_str()), (1, Some("awaiting_approval"), Some("pending")));
        set_policy(&st, json!({}));

        // rejected by the platform
        *f.mode.lock().unwrap() = "reject";
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("rej")).await.unwrap(), SendOutcome::Rejected(_)));
        assert_eq!(row("rej"), ("failed".into(), 1));
        assert_eq!(ev_for("rej")[0].1["delivery"], "failed");

        // unconfirmed
        *f.mode.lock().unwrap() = "uncertain";
        assert!(matches!(send(&st.db, &f, "user-a", "th-1", &req("unc")).await.unwrap(), SendOutcome::Unconfirmed { .. }));
        assert_eq!(row("unc"), ("unconfirmed".into(), 1));
        assert_eq!(ev_for("unc")[0].0, "channel.message.pending");

        // Slack reply path: success and failure both land on the ledger with the text
        let replies = |st_: &str| -> i64 {
            st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM channel_message_log WHERE direction='outbound' AND correlation_id LIKE 'reply%' AND state=?1", params![st_], |r| r.get(0)).unwrap()
        };
        *f.mode.lock().unwrap() = "ok";
        post_reply(&st.db, &f, &b, "1700000000.000100", "answer").await.unwrap();
        *f.mode.lock().unwrap() = "reject";
        assert!(post_reply(&st.db, &f, &b, "1700000000.000100", "answer 2").await.is_err());
        *f.mode.lock().unwrap() = "uncertain";
        assert!(post_reply(&st.db, &f, &b, "1700000000.000100", "answer 3").await.is_err());
        assert_eq!((replies("confirmed"), replies("failed"), replies("unconfirmed")), (1, 1, 1));
        let texts: i64 = st.db.connect().unwrap().query_row(
            "SELECT COUNT(*) FROM bot_events WHERE thread_id='th-1' AND json_extract(payload,'$.correlationId') LIKE 'reply%' AND json_extract(payload,'$.text') LIKE 'answer%'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(texts, 3);
    }

    #[tokio::test]
    async fn uncertain_delivery_is_unconfirmed_never_reposted_and_the_echo_confirms_it() {
        let st = setup("uncertain").await;
        let f = Fake { own: Some("UBOT".into()), ..Default::default() };
        let b = bound(&st, &f);
        *f.mode.lock().unwrap() = "uncertain";
        let req = SendReq { text: "did it land".into(), correlation_id: Some("c-9".into()), ..Default::default() };
        assert_eq!(send(&st.db, &f, "user-a", "th-1", &req).await.unwrap(), SendOutcome::Unconfirmed { correlation_id: "c-9".into() });
        assert_eq!(events(&st, "channel.message.pending"), 1);
        // A retry with the same correlation id reports state instead of posting twice.
        *f.mode.lock().unwrap() = "ok";
        assert_eq!(send(&st.db, &f, "user-a", "th-1", &req).await.unwrap(), SendOutcome::Replay { state: "unconfirmed".into(), remote_id: None });
        assert_eq!(f.posted.lock().unwrap().len(), 1);
        // Slack echoes our own message back.
        let echo = json!({ "type": "event_callback", "event": { "type": "message", "channel": "C1", "bot_id": "B1", "user": "UBOT", "text": "did it land", "ts": "1700000050.000300", "thread_ts": "1700000000.000100" } });
        assert_eq!(ingest_slack_side_event(&st.db, &f, &echo).unwrap(), Recorded::Echo);
        let (state, remote): (String, Option<String>) = st.db.connect().unwrap().query_row("SELECT state, remote_id FROM channel_message_log WHERE binding_id=?1 AND direction='outbound'", params![b.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((state.as_str(), remote.as_deref()), ("confirmed", Some("1700000050.000300")));
        // A definite rejection is reported as such.
        *f.mode.lock().unwrap() = "reject";
        let r = send(&st.db, &f, "user-a", "th-1", &SendReq { text: "no".into(), ..Default::default() }).await.unwrap();
        assert!(matches!(r, SendOutcome::Rejected(_)));
    }

    #[tokio::test]
    async fn reconnect_resumes_from_the_cursor_without_duplicates() {
        let st = setup("resume").await;
        let f = Fake::default();
        let b = bound(&st, &f);
        let first = ev(&f, &msg("1700000000.000100", None, "hi"));
        record_inbound(&st.db, &b, &first).unwrap();
        let second = ev(&f, &msg("1700000007.000200", Some("1700000000.000100"), "while offline"));
        let third = ev(&f, &msg("1700000009.000300", Some("1700000000.000100"), "also offline"));
        *f.history.lock().unwrap() = vec![first, second, third];
        assert_eq!(resume(&st.db, &f, &b).await.unwrap(), 2);
        assert_eq!(resume(&st.db, &f, &b).await.unwrap(), 0);
        assert_eq!(events(&st, "channel.message.received"), 3);
        assert_eq!(find_binding(&st.db, "slack", &b.conversation).unwrap().last_inbound.as_deref(), Some("1700000009.000300"));
    }

    #[tokio::test]
    async fn read_only_bindings_and_missing_bindings_refuse_to_post() {
        let st = setup("ro").await;
        let f = Fake::default();
        let req = SendReq { text: "x".into(), ..Default::default() };
        assert_eq!(send(&st.db, &f, "user-a", "th-1", &req).await.unwrap(), SendOutcome::NoBinding);
        let b = bound(&st, &f);
        st.db.connect().unwrap().execute("UPDATE channel_conversation_bindings SET read_only=1 WHERE id=?1", params![b.id]).unwrap();
        assert_eq!(send(&st.db, &f, "user-a", "th-1", &req).await.unwrap(), SendOutcome::ReadOnly);
    }
}
