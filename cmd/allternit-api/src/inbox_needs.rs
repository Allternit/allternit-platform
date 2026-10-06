//! The owner-level "Needs you" inbox: one list of everything across all of the
//! caller's bots, channels and vendor bots that is waiting on a person.
//!
//! Nothing is copied into an inbox table. Each source already holds its own
//! state, so the list is derived live and cannot drift from it:
//!
//! | kind          | source                                                    | inline action          |
//! |---------------|-----------------------------------------------------------|------------------------|
//! | `email_draft` | `agent_email_outbound` pending approval                   | approve / deny         |
//! | `approval`    | `gateway_approvals` pending (tools, vendor gates)         | approve / deny         |
//! | `held_send`   | the same, when the approval holds a channel post (the     | approve = send         |
//! |               | text is sent as soon as it is approved)                   |                        |
//! | `thread`      | `bot_threads` in `needs_you` / `review` with no approval  | open                   |
//! | `missed_call` | `call.ended` events that were missed                      | call back (client)     |
//! | `unanswered`  | latest inbound channel message nobody answered in N min   | reply (client)         |
//! | `failed_send` | `channel_message_log` outbound `failed` / `unconfirmed`   | open                   |
//! | `vendor_result` | `vendor_tickets` finished / failed / expired            | review (open)          |
//! | `draft`       | `inbox_items` `autonomy.draft` cards (autonomy level kept a draft) | review (open) |
//! | `factory_approval` | `factory_approvals` pending (a Factory node waiting on you) | approve / reject |
//!
//! `inbox_state` (V232) only remembers dismissals and snoozes. Routes live in
//! `inbox_routes::inbox_router` (`GET /inbox` carries `needsYou`).
//! Every action emits `inbox.changed` on the item's bot so other devices refresh.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

pub const DEFAULT_UNANSWERED_MINUTES: i64 = 30;
const WINDOW_DAYS: i64 = 7;
const TICKET_WINDOW_DAYS: i64 = 3;
const PER_SOURCE: i64 = 50;

pub fn needs_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/inbox/:id/approve", post(approve))
        .route("/inbox/:id/dismiss", post(dismiss))
        .route("/inbox/:id/snooze", post(snooze))
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok().map(|n| n.and_utc()))
}

fn snippet(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

#[allow(clippy::too_many_arguments)]
fn item(id: String, kind: &str, source: &str, bot_id: &str, bot_name: &str, thread_id: Option<&str>, person: Option<String>, summary: String, at: &str, primary: (&str, &str), approvable: bool) -> Value {
    let at = parse_ts(at).map(|d| d.to_rfc3339()).unwrap_or_else(|| at.to_string());
    let mut actions = vec!["dismiss", "snooze"];
    if approvable {
        actions.insert(0, "approve");
    }
    json!({
        "id": id,
        "kind": kind,
        "source": source,
        "botId": bot_id,
        "botName": bot_name,
        "threadId": thread_id,
        "person": person,
        "summary": summary,
        "at": at,
        "primary": { "action": primary.0, "label": primary.1 },
        "actions": actions,
    })
}

/// Every open item for `user_id`, newest first, with dismissed and snoozed ones removed.
/// `unanswered_min` is how long an inbound message may sit before it counts.
pub fn collect(conn: &Connection, user_id: &str, unanswered_min: i64) -> rusqlite::Result<Vec<Value>> {
    let now = Utc::now();
    let mut out: Vec<Value> = Vec::new();

    // Held outbound email.
    let mut st = conn.prepare(
        "SELECT o.id, o.agent_id, o.thread_id, o.to_address, o.subject, o.snippet, o.created_at, COALESCE(a.name, '')
         FROM agent_email_outbound o LEFT JOIN agents a ON a.id = o.agent_id
         WHERE o.user_id = ?1 AND o.status = 'pending_approval' ORDER BY o.created_at DESC LIMIT ?2",
    )?;
    for r in st.query_map(params![user_id, PER_SOURCE], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, String>(7)?))
    })? {
        let (id, bot, _thread, to, subject, snip, at, bot_name) = r?;
        let what = subject.filter(|s| !s.is_empty()).or(snip).unwrap_or_default();
        out.push(item(format!("email:{id}"), "email_draft", "email", &bot, &bot_name, None, Some(to.clone()), format!("Email to {to} is waiting for your OK: {}", snippet(&what, 120)), at.as_deref().unwrap_or(""), ("approve", "Send"), true));
    }

    // Approval gates (tools, vendor approvals, held channel posts).
    let mut st = conn.prepare(
        "SELECT g.id, g.bot_id, g.thread_id, g.authority, g.action, g.detail_json, g.created_at, COALESCE(a.name, ''), t.title,
                (SELECT b.provider FROM channel_conversation_bindings b WHERE b.thread_id = g.thread_id AND b.owner = g.owner ORDER BY b.created_at DESC LIMIT 1)
         FROM gateway_approvals g LEFT JOIN agents a ON a.id = g.bot_id LEFT JOIN bot_threads t ON t.id = g.thread_id
         WHERE g.owner = ?1 AND g.state = 'pending' ORDER BY g.created_at DESC LIMIT ?2",
    )?;
    for r in st.query_map(params![user_id, PER_SOURCE], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, Option<String>>(8)?, r.get::<_, Option<String>>(9)?))
    })? {
        let (id, bot, thread, authority, action, detail, at, bot_name, title, provider) = r?;
        let detail: Value = serde_json::from_str(&detail).unwrap_or(Value::Null);
        let held = authority == "allternit" && action.starts_with("post to ") && detail["text"].is_string();
        if held {
            let provider = provider.unwrap_or_else(|| action.trim_start_matches("post to ").to_string());
            out.push(item(format!("approval:{id}"), "held_send", &provider, &bot, &bot_name, Some(&thread), title.clone(), format!("{bot_name} wants to post on {provider}: {}", snippet(detail["text"].as_str().unwrap_or(""), 120)), &at, ("approve", "Send"), true));
        } else {
            out.push(item(format!("approval:{id}"), "approval", if authority == "vendor" { "vendor" } else { "tool" }, &bot, &bot_name, Some(&thread), title.clone(), format!("{bot_name} needs approval: {}", snippet(&action, 120)), &at, ("approve", "Approve"), true));
        }
    }

    // Threads waiting on the owner that have no approval card of their own.
    let mut st = conn.prepare(
        "SELECT t.id, t.bot_id, t.title, t.status, t.status_line, t.last_activity_at, COALESCE(a.name, '')
         FROM bot_threads t LEFT JOIN agents a ON a.id = t.bot_id
         WHERE t.user_id = ?1 AND t.status IN ('needs_you', 'review') AND t.incognito = 0
           AND NOT EXISTS (SELECT 1 FROM gateway_approvals g WHERE g.thread_id = t.id AND g.state = 'pending')
         ORDER BY t.last_activity_at DESC LIMIT ?2",
    )?;
    for r in st.query_map(params![user_id, PER_SOURCE], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?))
    })? {
        let (id, bot, title, status, line, at, bot_name) = r?;
        let why = line.filter(|l| !l.is_empty()).unwrap_or_else(|| if status == "review" { "ready for your review".into() } else { "needs you".into() });
        out.push(item(format!("thread:{id}"), "thread", "thread", &bot, &bot_name, Some(&id), None, format!("{title}: {}", snippet(&why, 120)), &at, ("open", if status == "review" { "Review" } else { "Open" }), false));
    }

    // Missed calls.
    let cutoff = now - Duration::days(WINDOW_DAYS);
    let mut st = conn.prepare(
        "SELECT e.id, e.bot_id, e.thread_id, e.payload, e.occurred_at, COALESCE(a.name, '')
         FROM bot_events e JOIN agents a ON a.id = e.bot_id
         WHERE a.user_id = ?1 AND e.event_type = 'call.ended'
         ORDER BY e.rowid DESC LIMIT 400",
    )?;
    let mut missed = 0;
    for r in st.query_map(params![user_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))
    })? {
        let (id, bot, thread, payload, at, bot_name) = r?;
        let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let is_missed = p["missed"].as_bool() == Some(true) || p["answered"].as_bool() == Some(false) || matches!(p["reason"].as_str(), Some("missed" | "no_answer" | "voicemail"));
        if !is_missed || parse_ts(&at).is_some_and(|d| d < cutoff) {
            continue;
        }
        let who = p["from"].as_str().or_else(|| p["caller"].as_str()).or_else(|| p["number"].as_str()).map(str::to_string);
        let voicemail = p["reason"].as_str() == Some("voicemail") || p["voicemail"].as_bool() == Some(true);
        let label = who.clone().unwrap_or_else(|| "Someone".into());
        let summary = if voicemail { format!("{label} left a voicemail for {bot_name}") } else { format!("Missed call from {label} to {bot_name}") };
        out.push(item(format!("call:{id}"), "missed_call", "call", &bot, &bot_name, thread.as_deref(), who, summary, &at, ("callback", "Call back"), false));
        missed += 1;
        if missed >= PER_SOURCE {
            break;
        }
    }

    // Latest inbound message per thread that no one answered.
    let mut st = conn.prepare(
        "SELECT e.id, e.bot_id, e.thread_id, e.payload, e.occurred_at, COALESCE(a.name, ''), t.title
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.event_type = 'channel.message.received' AND e.thread_id IS NOT NULL
           AND e.payload NOT LIKE '%\"own\":true%'
           AND e.rowid = (SELECT MAX(x.rowid) FROM bot_events x WHERE x.thread_id = e.thread_id AND x.event_type = 'channel.message.received' AND x.payload NOT LIKE '%\"own\":true%')
           AND NOT EXISTS (SELECT 1 FROM bot_events r WHERE r.thread_id = e.thread_id AND r.rowid > e.rowid
                           AND (r.event_type = 'channel.message.sent' OR (r.event_type = 'channel.message.received' AND r.payload LIKE '%\"own\":true%')))
         ORDER BY e.rowid DESC LIMIT 200",
    )?;
    let mut unanswered = 0;
    for r in st.query_map(params![user_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, Option<String>>(6)?))
    })? {
        let (_msg, bot, thread, payload, at, bot_name, title) = r?;
        let when = parse_ts(&at);
        if when.is_some_and(|d| d < cutoff || now - d < Duration::minutes(unanswered_min)) {
            continue;
        }
        let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let who = p["from"].as_str().or_else(|| p["user"].as_str()).map(str::to_string).or(title);
        let provider = p["provider"].as_str().or_else(|| p["channel"].as_str()).unwrap_or("channel");
        let text = snippet(p["text"].as_str().unwrap_or(""), 100);
        let label = who.clone().unwrap_or_else(|| "Someone".into());
        out.push(item(format!("reply:{thread}"), "unanswered", provider, &bot, &bot_name, Some(&thread), who, format!("{label} on {provider} is waiting for a reply: {text}"), &at, ("reply", "Reply"), false));
        unanswered += 1;
        if unanswered >= PER_SOURCE {
            break;
        }
    }

    // Failed or unconfirmed deliveries.
    let mut st = conn.prepare(
        "SELECT l.id, l.thread_id, l.state, l.detail_json, l.updated_at, t.bot_id, COALESCE(a.name, ''), b.provider, t.title
         FROM channel_message_log l
         JOIN bot_threads t ON t.id = l.thread_id
         LEFT JOIN agents a ON a.id = t.bot_id
         LEFT JOIN channel_conversation_bindings b ON b.id = l.binding_id
         WHERE l.owner = ?1 AND l.direction = 'outbound' AND l.state IN ('failed', 'unconfirmed')
         ORDER BY l.updated_at DESC LIMIT ?2",
    )?;
    for r in st.query_map(params![user_id, PER_SOURCE], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?))
    })? {
        let (id, thread, state, detail, at, bot, bot_name, provider, title) = r?;
        if parse_ts(&at).is_some_and(|d| d < cutoff) {
            continue;
        }
        let d: Value = serde_json::from_str(&detail).unwrap_or(Value::Null);
        let provider = provider.unwrap_or_else(|| "channel".into());
        let verb = if state == "failed" { "did not go through" } else { "may not have gone through" };
        out.push(item(format!("failed:{id}"), "failed_send", &provider, &bot, &bot_name, Some(&thread), title, format!("A message from {bot_name} on {provider} {verb}: {}", snippet(d["text"].as_str().unwrap_or(""), 100)), &at, ("open", "Open"), false));
    }

    // Vendor ticket results.
    let mut st = conn.prepare(
        "SELECT v.id, v.vendor_bot_id, v.thread_id, v.status, v.instructions, v.result_json, v.error, v.updated_at, COALESCE(a.name, '')
         FROM vendor_tickets v LEFT JOIN agents a ON a.id = v.vendor_bot_id
         WHERE v.owner = ?1 AND v.status IN ('done', 'failed', 'expired')
         ORDER BY v.updated_at DESC LIMIT ?2",
    )?;
    let ticket_cutoff = now - Duration::days(TICKET_WINDOW_DAYS);
    for r in st.query_map(params![user_id, PER_SOURCE], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?))
    })? {
        let (id, bot, thread, status, instr, result, error, at, bot_name) = r?;
        if parse_ts(&at).is_some_and(|d| d < ticket_cutoff) {
            continue;
        }
        let result: Value = result.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let summary = match status.as_str() {
            "done" => format!("{bot_name} finished {id}: {}", snippet(result["summary"].as_str().unwrap_or(&instr), 120)),
            "failed" => format!("{bot_name} could not finish {id}: {}", snippet(error.as_deref().unwrap_or("it failed"), 120)),
            _ => format!("{bot_name} did not answer {id} in time"),
        };
        out.push(item(format!("ticket:{id}"), "vendor_result", "vendor", &bot, &bot_name, Some(&thread), None, summary, &at, ("open", "Review"), false));
    }

    // Drafts a bot's autonomy level kept instead of sending (`autonomy.draft` cards).
    let mut st = conn.prepare(
        "SELECT i.id, COALESCE(i.agent_id, ''), COALESCE(i.title, ''), COALESCE(i.body, ''), i.metadata, COALESCE(i.created_at, ''), COALESCE(a.name, '')
         FROM inbox_items i LEFT JOIN agents a ON a.id = i.agent_id
         WHERE i.user_id = ?1 AND i.type = 'autonomy.draft' AND i.status = 'unread'
         ORDER BY i.created_at DESC LIMIT ?2",
    )?;
    let draft_cutoff = now - Duration::days(WINDOW_DAYS);
    for r in st.query_map(params![user_id, PER_SOURCE], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?))
    })? {
        let (id, bot, title, body, meta, at, bot_name) = r?;
        if parse_ts(&at).is_some_and(|d| d < draft_cutoff) {
            continue;
        }
        let meta: Value = meta.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null);
        let source = meta["channel"].as_str().unwrap_or("autonomy").to_string();
        let person = meta["to"].as_str().map(str::to_string);
        // The card body is "<reason>\n\n<preview>"; the preview is what the person would get.
        let preview = body.rsplit("\n\n").next().unwrap_or(&body);
        let summary = if preview.trim().is_empty() { title } else { format!("{title}: {}", snippet(preview, 120)) };
        out.push(item(format!("draft:{id}"), "draft", &source, &bot, &bot_name, meta["threadId"].as_str(), person, summary, &at, ("open", "Review"), false));
    }

    // Factory nodes waiting on the owner (wait-gates and judge NEEDS_HUMAN).
    for a in crate::factory_approvals::inbox_items(conn, user_id)? {
        let bot = a.bot_id.clone().unwrap_or_default();
        let bot_name: String = if bot.is_empty() { String::new() } else { conn.query_row("SELECT COALESCE(name, '') FROM agents WHERE id = ?1", params![bot], |r| r.get(0)).optional()?.unwrap_or_default() };
        let summary = if a.summary.is_empty() { format!("Factory: {} needs your OK", a.title) } else { format!("Factory: {} needs your OK: {}", a.title, snippet(&a.summary, 120)) };
        let mut it = item(format!("factory:{}", a.id), "factory_approval", "factory", &bot, &bot_name, None, None, summary, &a.created_at, ("approve", "Approve"), true);
        it["approval"] = a.to_json();
        out.push(it);
    }

    // Drop what the owner already handled (unless the item changed since).
    let mut st = conn.prepare("SELECT item_id, state, until, item_at FROM inbox_state WHERE owner = ?1")?;
    let handled: std::collections::HashMap<String, (String, Option<String>, String)> = st
        .query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?))))?
        .filter_map(Result::ok)
        .collect();
    out.retain(|it| {
        let Some((state, until, item_at)) = handled.get(it["id"].as_str().unwrap_or("")) else { return true };
        let cur = parse_ts(it["at"].as_str().unwrap_or(""));
        let seen = parse_ts(item_at);
        let changed = matches!((cur, seen), (Some(c), Some(s)) if c > s);
        if changed {
            return true;
        }
        match state.as_str() {
            "snoozed" => until.as_deref().and_then(parse_ts).is_some_and(|u| u <= now),
            _ => false,
        }
    });
    out.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
    Ok(out)
}

fn err(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(json!({ "error": msg, "code": code }))).into_response()
}

fn find(state: &AppState, user_id: &str, id: &str) -> Result<Value, Response> {
    let conn = state.db.connect().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e.to_string()))?;
    let all = collect(&conn, user_id, DEFAULT_UNANSWERED_MINUTES).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e.to_string()))?;
    all.into_iter().find(|i| i["id"] == id).ok_or_else(|| err(StatusCode::NOT_FOUND, "NOT_FOUND", "this item is already resolved"))
}

fn changed(state: &AppState, it: &Value, action: &str) {
    let (Some(bot), Some(id)) = (it["botId"].as_str(), it["id"].as_str()) else { return };
    crate::gateway_runner::led(&state.db, bot, it["threadId"].as_str().unwrap_or(""), None, "inbox.changed", ("user", "owner"), json!({ "itemId": id, "action": action }), None);
}

fn remember(state: &AppState, user_id: &str, it: &Value, st: &str, until: Option<String>) -> rusqlite::Result<()> {
    let conn = state.db.connect()?;
    conn.execute(
        "INSERT INTO inbox_state (owner, item_id, state, until, item_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(owner, item_id) DO UPDATE SET state = ?3, until = ?4, item_at = ?5, updated_at = ?6",
        params![user_id, it["id"].as_str().unwrap_or(""), st, until, it["at"].as_str().unwrap_or(""), Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

#[derive(Deserialize, Default)]
struct ActBody {
    /// `approve` (default) or `deny`.
    decision: Option<String>,
    /// Snooze length; default 60.
    minutes: Option<i64>,
}

fn body(b: Option<Json<ActBody>>) -> ActBody {
    b.map(|Json(b)| b).unwrap_or_default()
}

async fn approve(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, b: Option<Json<ActBody>>) -> Response {
    let b = body(b);
    let approve = match b.decision.as_deref() {
        None | Some("approve") => true,
        Some("deny") => false,
        _ => return err(StatusCode::BAD_REQUEST, "BAD_DECISION", "decision must be approve or deny"),
    };
    let it = match find(&state, &user.user_id, &id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let result: Result<Value, Response> = match it["kind"].as_str().unwrap_or("") {
        "email_draft" => {
            let oid = id.trim_start_matches("email:");
            let thread: Option<String> = state.db.connect().ok().and_then(|c| c.query_row("SELECT thread_id FROM agent_email_outbound WHERE id = ?1 AND user_id = ?2", params![oid, user.user_id], |r| r.get(0)).optional().ok().flatten());
            match thread {
                None => Err(err(StatusCode::NOT_FOUND, "NOT_FOUND", "this email is already handled")),
                Some(t) => match crate::agent_email_routes::decide_outbound_for_thread(&state, &t, approve).await {
                    crate::agent_email_routes::EmailDecisionOutcome::Applied => Ok(json!({ "state": if approve { "sent" } else { "rejected" } })),
                    crate::agent_email_routes::EmailDecisionOutcome::NotEmailThread => Err(err(StatusCode::NOT_FOUND, "NOT_FOUND", "this email is already handled")),
                    crate::agent_email_routes::EmailDecisionOutcome::Failed(e) => Err(err(StatusCode::BAD_GATEWAY, "EMAIL_DECISION_FAILED", &e)),
                },
            }
        }
        "approval" | "held_send" => {
            let aid = id.trim_start_matches("approval:").to_string();
            let tx = crate::gateway_runner::transport(&state);
            match crate::gateway_runner::respond_approval(&state.db, tx.as_ref(), &user.user_id, &aid, if approve { "approve" } else { "deny" }, ("user", &user.user_id)).await {
                Err(e) => Err(e.into_response()),
                Ok(v) if approve && it["kind"] == "held_send" => Ok(json!({ "state": "approved", "send": send_held(&state, &user.user_id, &aid).await, "approval": v })),
                Ok(v) => Ok(v),
            }
        }
        "factory_approval" => {
            let aid = id.trim_start_matches("factory:");
            match crate::factory_approvals::resolve(&state, aid, approve, &user.user_id, crate::factory_approvals::Provenance::app(), None).await {
                Ok(a) => Ok(json!({ "state": a.state, "approval": a.to_json() })),
                Err(e) => Err(e.into_response()),
            }
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "NOT_APPROVABLE", "open this item to deal with it, or dismiss it")),
    };
    match result {
        Ok(v) => {
            changed(&state, &it, "approve");
            Json(json!({ "id": id, "result": v })).into_response()
        }
        Err(r) => r,
    }
}

/// Post a held channel message now that its approval is granted. The same correlation id and
/// the approval id go back through `channel_gateway::send`, so it is posted once and the
/// approval is consumed.
async fn send_held(state: &Arc<AppState>, owner: &str, approval_id: &str) -> Value {
    let row: Option<(String, Option<String>, String)> = state.db.connect().ok().and_then(|c| {
        c.query_row("SELECT thread_id, correlation_id, detail_json FROM gateway_approvals WHERE id = ?1 AND owner = ?2", params![approval_id, owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().ok().flatten()
    });
    let Some((thread, corr, detail)) = row else { return json!({ "state": "unknown" }) };
    let text = serde_json::from_str::<Value>(&detail).ok().and_then(|d| d["text"].as_str().map(str::to_string)).unwrap_or_default();
    let Some(binding) = crate::channel_gateway::binding_for_thread(&state.db, owner, &thread) else { return json!({ "state": "no_binding" }) };
    let Some(tx) = crate::channel_transports::transport_for(state, &binding) else { return json!({ "state": "channel_offline", "provider": binding.provider }) };
    let req = crate::channel_gateway::SendReq { text, posting_identity_id: None, correlation_id: corr, consequential: Some(true), allternit_approval_id: Some(approval_id.to_string()), files: vec![] };
    match crate::channel_gateway::send(&state.db, tx.as_ref(), owner, &thread, &req).await {
        Ok(crate::channel_gateway::SendOutcome::Sent { .. }) | Ok(crate::channel_gateway::SendOutcome::Replay { .. }) => json!({ "state": "sent" }),
        Ok(crate::channel_gateway::SendOutcome::Unconfirmed { .. }) => json!({ "state": "unconfirmed" }),
        Ok(crate::channel_gateway::SendOutcome::Rejected(m)) => json!({ "state": "rejected", "error": m }),
        Ok(_) => json!({ "state": "not_sent" }),
        Err(e) => json!({ "state": "error", "error": e }),
    }
}

async fn dismiss(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let it = match find(&state, &user.user_id, &id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    if let Err(e) = remember(&state, &user.user_id, &it, "dismissed", None) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e.to_string());
    }
    changed(&state, &it, "dismiss");
    Json(json!({ "id": id, "state": "dismissed" })).into_response()
}

async fn snooze(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, b: Option<Json<ActBody>>) -> Response {
    let minutes = body(b).minutes.unwrap_or(60).clamp(1, 60 * 24 * 14);
    let it = match find(&state, &user.user_id, &id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let until = (Utc::now() + Duration::minutes(minutes)).to_rfc3339();
    if let Err(e) = remember(&state, &user.user_id, &it, "snoozed", Some(until.clone())) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e.to_string());
    }
    changed(&state, &it, "snooze");
    Json(json!({ "id": id, "state": "snoozed", "until": until })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-inbox-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        for (id, u, n) in [("bot-a", "user-a", "Ledger"), ("bot-b", "user-b", "Other")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, ?2, ?3, 'm', 'p', 1, '{}')", params![id, u, n]).unwrap();
        }
        state
    }

    fn user(id: &str) -> AuthUser {
        AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    fn ago(min: i64) -> String {
        (Utc::now() - Duration::minutes(min)).to_rfc3339()
    }

    fn thread(c: &Connection, id: &str, bot: &str, user: &str, status: &str, line: &str) {
        let t = ago(1);
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, status_line, last_activity_at, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7,?7)", params![id, user, bot, format!("Thread {id}"), status, line, t]).unwrap();
    }

    fn ev(c: &Connection, bot: &str, seq: i64, thread: &str, ty: &str, payload: Value, at: &str) {
        c.execute(
            "INSERT INTO bot_events (id, bot_id, seq, event_type, actor_type, actor_id, payload, occurred_at, thread_id) VALUES (?1, ?2, ?3, ?4, 'user', 'x', ?5, ?6, ?7)",
            params![format!("e{bot}{seq}"), bot, seq, ty, payload.to_string(), at, thread],
        )
        .unwrap();
    }

    async fn call(app: &Router, user_id: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(Request::builder().method("POST").uri(path).header("content-type", "application/json").extension(user(user_id)).body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn kinds(items: &[Value]) -> Vec<String> {
        let mut k: Vec<String> = items.iter().map(|i| i["kind"].as_str().unwrap().to_string()).collect();
        k.sort();
        k
    }

    #[tokio::test]
    async fn collects_every_source_scoped_to_the_owner() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        thread(&c, "t-wait", "bot-a", "user-a", "needs_you", "pick a date");
        thread(&c, "t-appr", "bot-a", "user-a", "needs_you", "Approval needed");
        thread(&c, "t-msg", "bot-a", "user-a", "idle", "");
        thread(&c, "t-other", "bot-b", "user-b", "needs_you", "not yours");
        c.execute("INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, authority, action, detail_json, state, created_at) VALUES ('ap1','user-a','t-appr','bot-a','allternit','run shell','{}','pending',?1)", params![ago(5)]).unwrap();
        c.execute("INSERT INTO agent_email_outbound (id, agent_id, user_id, thread_id, idempotency_key, to_address, subject, status) VALUES ('o1','bot-a','user-a','mail:email-out-o1','k1','sam@example.com','Quote','pending_approval')", []).unwrap();
        ev(&c, "bot-a", 1, "t-msg", "call.ended", json!({ "reason": "no_answer", "from": "+15550001" }), &ago(10));
        ev(&c, "bot-a", 2, "t-msg", "call.ended", json!({ "reason": "hangup" }), &ago(10));
        ev(&c, "bot-a", 3, "t-msg", "channel.message.received", json!({ "text": "are you there?", "from": "Sam", "provider": "telegram" }), &ago(45));
        ev(&c, "bot-b", 1, "t-other", "channel.message.received", json!({ "text": "x" }), &ago(45));
        c.execute("INSERT INTO vendor_tickets (owner, id, n, vendor_bot_id, thread_id, instructions, status, deadline_at, created_at, updated_at) VALUES ('user-a','T-1',1,'bot-a','t-msg','do it','done','x',?1,?1)", params![ago(20)]).unwrap();
        c.execute("INSERT INTO inbox_items (id, user_id, agent_id, type, title, body, severity, status, metadata, created_at) VALUES ('d1','user-a','bot-a','autonomy.draft','Ada drafted a text for Sam','Draft only on this channel\n\nOn my way','info','unread',?1,?2)",
            params![json!({ "channel": "telegram", "to": "Sam", "threadId": "t-msg" }).to_string(), ago(15)]).unwrap();
        c.execute("INSERT INTO inbox_items (id, user_id, agent_id, type, title, status, created_at) VALUES ('d2','user-a','bot-a','autonomy.digest','sent','unread',?1)", params![ago(15)]).unwrap();
        let items = collect(&c, "user-a", 30).unwrap();
        assert_eq!(kinds(&items), ["approval", "draft", "email_draft", "missed_call", "thread", "unanswered", "vendor_result"]);
        let d = items.iter().find(|i| i["kind"] == "draft").unwrap();
        assert_eq!((d["source"].as_str(), d["person"].as_str(), d["threadId"].as_str()), (Some("telegram"), Some("Sam"), Some("t-msg")));
        assert_eq!(d["summary"], "Ada drafted a text for Sam: On my way");
        let un = items.iter().find(|i| i["kind"] == "unanswered").unwrap();
        assert_eq!(un["person"], "Sam");
        assert_eq!(un["primary"]["action"], "reply");
        assert!(kinds(&collect(&c, "user-b", 30).unwrap()).contains(&"thread".to_string()));
        assert_eq!(collect(&c, "user-b", 30).unwrap().len(), 2);
        // Newest first.
        let ats: Vec<_> = items.iter().map(|i| i["at"].as_str().unwrap().to_string()).collect();
        let mut sorted = ats.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(ats, sorted);
    }

    #[tokio::test]
    async fn a_message_is_not_unanswered_when_young_or_replied_to() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        thread(&c, "t1", "bot-a", "user-a", "idle", "");
        thread(&c, "t2", "bot-a", "user-a", "idle", "");
        ev(&c, "bot-a", 1, "t1", "channel.message.received", json!({ "text": "fresh" }), &ago(5));
        ev(&c, "bot-a", 2, "t2", "channel.message.received", json!({ "text": "old but answered" }), &ago(90));
        ev(&c, "bot-a", 3, "t2", "channel.message.sent", json!({ "text": "reply" }), &ago(80));
        assert!(collect(&c, "user-a", 30).unwrap().is_empty());
        // A shorter threshold counts the fresh one.
        assert_eq!(collect(&c, "user-a", 1).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn held_channel_post_and_failed_delivery_and_ticket_failure() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        thread(&c, "t1", "bot-a", "user-a", "needs_you", "Approval needed (allternit): post to slack");
        c.execute("INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, authority, action, detail_json, correlation_id, state, created_at) VALUES ('ap1','user-a','t1','bot-a','allternit','post to slack','{\"text\":\"ship it\"}','c1','pending',?1)", params![ago(2)]).unwrap();
        c.execute("INSERT INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, state, detail_json, created_at, updated_at) VALUES ('l1','user-a','b1','t1','outbound','message','failed','{\"text\":\"hello\"}',?1,?1)", params![ago(3)]).unwrap();
        c.execute("INSERT INTO vendor_tickets (owner, id, n, vendor_bot_id, thread_id, instructions, status, error, deadline_at, created_at, updated_at) VALUES ('user-a','T-2',2,'bot-a','t1','do it','failed','vendor said no','x',?1,?1)", params![ago(3)]).unwrap();
        let items = collect(&c, "user-a", 30).unwrap();
        assert_eq!(kinds(&items), ["failed_send", "held_send", "vendor_result"]);
        let held = items.iter().find(|i| i["kind"] == "held_send").unwrap();
        assert!(held["summary"].as_str().unwrap().contains("ship it"));
        assert_eq!(held["primary"]["label"], "Send");
        let failed = items.iter().find(|i| i["kind"] == "failed_send").unwrap();
        assert!(failed["summary"].as_str().unwrap().contains("did not go through"));
    }

    #[tokio::test]
    async fn dismiss_and_snooze_hide_until_the_item_changes() {
        let st = setup().await;
        {
            let c = st.db.connect().unwrap();
            thread(&c, "t1", "bot-a", "user-a", "needs_you", "decide");
            thread(&c, "t2", "bot-a", "user-a", "review", "check");
        }
        let app = needs_router().with_state(st.clone());
        let (s, v) = call(&app, "user-a", "/inbox/thread:t1/dismiss", json!({})).await;
        assert_eq!((s, v["state"].as_str()), (StatusCode::OK, Some("dismissed")));
        let (s, v) = call(&app, "user-a", "/inbox/thread:t2/snooze", json!({ "minutes": 30 })).await;
        assert_eq!((s, v["state"].as_str()), (StatusCode::OK, Some("snoozed")));
        let c = st.db.connect().unwrap();
        assert!(collect(&c, "user-a", 30).unwrap().is_empty());
        // The inbox change reached the events ledger for the item's bot.
        let n: i64 = c.query_row("SELECT COUNT(*) FROM bot_events WHERE event_type = 'inbox.changed' AND bot_id = 'bot-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        // The thread asks for the owner again later: it comes back.
        c.execute("UPDATE bot_threads SET last_activity_at = ?1 WHERE id = 't1'", params![(Utc::now() + Duration::minutes(5)).to_rfc3339()]).unwrap();
        let back: Vec<_> = collect(&c, "user-a", 30).unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(back, ["thread:t1"]);
        // An expired snooze also comes back.
        c.execute("UPDATE inbox_state SET until = ?1 WHERE item_id = 'thread:t2'", params![ago(1)]).unwrap();
        assert_eq!(collect(&c, "user-a", 30).unwrap().len(), 2);
        // Another user's item is not reachable.
        let (s, _) = call(&app, "user-b", "/inbox/thread:t1/dismiss", json!({})).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn approve_gates_and_resolves_a_gateway_approval() {
        let st = setup().await;
        {
            let c = st.db.connect().unwrap();
            thread(&c, "t1", "bot-a", "user-a", "needs_you", "Approval needed");
            thread(&c, "t2", "bot-a", "user-a", "needs_you", "just waiting");
            c.execute("INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, authority, action, detail_json, state, created_at) VALUES ('ap1','user-a','t1','bot-a','allternit','delete files','{}','pending',?1)", params![ago(2)]).unwrap();
        }
        let app = needs_router().with_state(st.clone());
        // A plain waiting thread cannot be approved from the inbox.
        let (s, v) = call(&app, "user-a", "/inbox/thread:t2/approve", json!({})).await;
        assert_eq!((s, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("NOT_APPROVABLE")));
        // Someone else's approval is invisible.
        let (s, _) = call(&app, "user-b", "/inbox/approval:ap1/approve", json!({})).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, v) = call(&app, "user-a", "/inbox/approval:ap1/approve", json!({ "decision": "maybe" })).await;
        assert_eq!((s, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("BAD_DECISION")));
        let (s, v) = call(&app, "user-a", "/inbox/approval:ap1/approve", json!({})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["result"]["state"], "approved");
        let c = st.db.connect().unwrap();
        let state: String = c.query_row("SELECT state FROM gateway_approvals WHERE id = 'ap1'", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "approved");
        let ids: Vec<_> = collect(&c, "user-a", 30).unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, ["thread:t2"]);
        // Already resolved: a second tap is a clean 404, not a double send.
        let (s, _) = call(&app, "user-a", "/inbox/approval:ap1/approve", json!({})).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn deny_rejects_the_approval() {
        let st = setup().await;
        {
            let c = st.db.connect().unwrap();
            thread(&c, "t1", "bot-a", "user-a", "needs_you", "Approval needed");
            c.execute("INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, authority, action, detail_json, state, created_at) VALUES ('ap1','user-a','t1','bot-a','allternit','wire money','{}','pending',?1)", params![ago(2)]).unwrap();
        }
        let app = needs_router().with_state(st.clone());
        let (s, _) = call(&app, "user-a", "/inbox/approval:ap1/approve", json!({ "decision": "deny" })).await;
        assert_eq!(s, StatusCode::OK);
        let c = st.db.connect().unwrap();
        let state: String = c.query_row("SELECT state FROM gateway_approvals WHERE id = 'ap1'", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "denied");
    }
}
