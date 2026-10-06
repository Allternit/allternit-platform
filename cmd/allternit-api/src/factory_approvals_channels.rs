//! Channel side of Factory approvals: Telegram, Slack, SMS and email.
//!
//! * **Only the owner's own accounts.** A request goes to accounts in
//!   `provider_account_bindings` owned by the approval's owner (preferring one
//!   linked to the node's bot in `channel_account_bots`), and only to the
//!   owner's **verified identity** on that account:
//!   - Telegram: the managed-pairing owner (`tg_owner_user_id`), or a
//!     `verify` pairing (below).
//!   - Slack, SMS: a `verify` pairing: the app issues a code
//!     (`POST /api/factory/approvals/identities`) and the owner sends
//!     `verify <code>` to the bot from their own Slack user / phone.
//!   - Email: the owner's account email (`users.email`), sent from one of the
//!     owner's bots that has an email address; DMARC and loop guards apply.
//! * **Telegram** gets Approve / Reject buttons; **Slack, SMS, email** get the
//!   text `Reply "approve <node> <code>" or "reject <node> <code>"`. (Slack
//!   buttons need the cloud to relay Slack interactivity; until then Slack
//!   uses the reply text.)
//! * **Inbound** answers are accepted only when the sender is the verified
//!   owner identity on that account, the message isn't forwarded, the whole
//!   message is exactly the reply, and the code matches, is unused and
//!   unexpired. Every refusal is logged with its reason
//!   (`factory_approval_refusals`).
//! * **After a decision** every surface's request is edited (Telegram) or
//!   followed up (Slack, SMS, email) with "Approved by Eoj in Telegram, 14:02 UTC".

use std::sync::Arc;

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::channel_gateway::Outbound;
use crate::channel_transports::{accounts, build_transport, pick, Account, HttpReq, HttpSend, ReqwestSend};
use crate::factory_approvals::{self as fa, api_err, check_code, issue_code, parse_reply, refuse, reply_instructions, use_code, Approval, CodeCheck, Provenance, ResolveError};
use crate::AppState;

/// Channel accounts that can carry approvals (email is not an account binding).
pub const ACCOUNT_CHANNELS: [&str; 3] = ["telegram", "slack", "sms"];
const VERIFY_TTL_MINUTES: i64 = 15;

// ---------------------------------------------------------------- http seam

static HTTP: Lazy<std::sync::RwLock<Option<Arc<dyn HttpSend>>>> = Lazy::new(|| std::sync::RwLock::new(None));

/// The HTTP client for approval posts (tests swap it with [`set_http`]).
pub fn http() -> Arc<dyn HttpSend> {
    HTTP.read().ok().and_then(|h| h.clone()).unwrap_or_else(|| Arc::new(ReqwestSend))
}

#[cfg(test)]
pub fn set_http(h: Option<Arc<dyn HttpSend>>) {
    *HTTP.write().unwrap() = h;
}

// ---------------------------------------------------------------- identities

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// The owner's verified identity on `account_id` for `channel`, if any.
pub fn owner_identity(conn: &Connection, account_id: &str, channel: &str, owner: &str) -> Option<String> {
    let stored: Option<String> = conn
        .query_row("SELECT identity FROM factory_owner_identities WHERE account_id = ?1 AND channel = ?2 AND owner = ?3", params![account_id, channel, owner], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    if stored.is_some() || channel != "telegram" {
        return stored;
    }
    conn.query_row(
        "SELECT tg_owner_user_id FROM provider_account_bindings WHERE id = ?1 AND owner = ?2 AND vendor = 'telegram'",
        params![account_id, owner],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
    .filter(|s| !s.trim().is_empty())
}

fn owner_email(conn: &Connection, owner: &str) -> Option<String> {
    conn.query_row("SELECT email FROM users WHERE id = ?1", params![owner], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten().flatten().filter(|e| e.contains('@'))
}

fn owner_name(conn: &Connection, owner: &str) -> String {
    conn.query_row("SELECT COALESCE(NULLIF(name, ''), email) FROM users WHERE id = ?1", params![owner], |r| r.get::<_, Option<String>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
        .unwrap_or_else(|| "the owner".to_string())
}

/// The owner's accounts for `channel`, the node's bot's linked account first.
fn owner_accounts(state: &AppState, owner: &str, channel: &str, bot: Option<&str>) -> Vec<Account> {
    let mut list: Vec<Account> = accounts(&state.db, channel, None).into_iter().filter(|a| a.owner == owner).collect();
    if let (Some(bot), Ok(conn)) = (bot, state.db.connect()) {
        let linked: Vec<String> = conn
            .prepare("SELECT account_id FROM channel_account_bots WHERE bot_id = ?1 AND owner = ?2")
            .and_then(|mut q| q.query_map(params![bot, owner], |r| r.get(0))?.collect())
            .unwrap_or_default();
        list.sort_by_key(|a| !linked.contains(&a.id));
    }
    list
}

// ---------------------------------------------------------------- outbound

fn request_text(a: &Approval, code: &str, buttons: bool) -> String {
    let lead = if buttons { "Tap Approve or Reject, or reply" } else { "Reply" };
    let how = reply_instructions(&a.node_id, code);
    let how = how.strip_prefix("Reply").map(|rest| format!("{lead}{rest}")).unwrap_or(how);
    format!("Allternit Factory: \"{}\" needs your OK.\n{}\n\n{how}", a.title, a.summary)
}

fn record_message(conn: &Connection, a: &Approval, channel: &str, account: Option<&str>, chat: &str, message: &str) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO factory_approval_messages (approval_id, channel, account_id, chat_id, message_id, state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'sent', ?6)",
        params![a.id, channel, account, chat, message, now().to_rfc3339()],
    );
}

fn drop_code(conn: &Connection, approval_id: &str, channel: &str) {
    let _ = conn.execute("DELETE FROM factory_approval_codes WHERE approval_id = ?1 AND channel = ?2", params![approval_id, channel]);
}

async fn telegram_send(http: &Arc<dyn HttpSend>, token: &str, chat: &str, text: &str, markup: Option<Value>) -> Result<String, String> {
    let mut body = json!({ "chat_id": chat, "text": text });
    if let Some(m) = markup {
        body["reply_markup"] = m;
    }
    let r = http.post_json(HttpReq { url: format!("https://api.telegram.org/bot{token}/sendMessage"), headers: vec![], body }).await?;
    if r.body.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(format!("Telegram sendMessage failed ({}): {}", r.status, r.body.get("description").and_then(Value::as_str).unwrap_or("unexpected reply")));
    }
    r.body.pointer("/result/message_id").and_then(|v| v.as_i64().map(|n| n.to_string())).ok_or_else(|| "Telegram returned no message id".into())
}

/// Send the request to every channel the owner can answer on. Returns the
/// channels it actually reached (each with its own one-time code).
pub async fn send_requests(state: &Arc<AppState>, a: &Approval) -> Vec<String> {
    let http = http();
    let mut sent = Vec::new();
    for channel in ACCOUNT_CHANNELS {
        for acct in owner_accounts(state, &a.owner, channel, a.bot_id.as_deref()) {
            let (identity, code) = {
                let Ok(conn) = state.db.connect() else { continue };
                let Some(identity) = owner_identity(&conn, &acct.id, channel, &a.owner) else {
                    tracing::info!(approval = %a.id, channel, account = %acct.id, "factory approval: no verified owner identity on this account");
                    continue;
                };
                let Ok(code) = issue_code(&conn, &a.id, channel, Some(&acct.id), now()) else { continue };
                (identity, code)
            };
            let result: Result<String, String> = if channel == "telegram" {
                let token = pick(&acct.secret, "botToken");
                let markup = json!({ "inline_keyboard": [[
                    { "text": "Approve", "callback_data": format!("fa:{}:a:{code}", a.id) },
                    { "text": "Reject", "callback_data": format!("fa:{}:r:{code}", a.id) },
                ]] });
                if token.is_empty() { Err("no bot token".into()) } else { telegram_send(&http, &token, &identity, &request_text(a, &code, true), Some(markup)).await }
            } else {
                match build_transport(channel, &acct.secret, http.clone()) {
                    None => Err("no transport".into()),
                    Some(tx) => tx
                        .post(&Outbound { workspace: None, channel: identity.clone(), thread: (channel == "sms").then(|| identity.clone()), text: request_text(a, &code, false), identity: None })
                        .await
                        .map(|r| r.remote_id)
                        .map_err(|e| format!("{e:?}")),
                }
            };
            let Ok(conn) = state.db.connect() else { continue };
            match result {
                Ok(message) => {
                    record_message(&conn, a, channel, Some(&acct.id), &identity, &message);
                    sent.push(channel.to_string());
                    break;
                }
                Err(e) => {
                    drop_code(&conn, &a.id, channel);
                    tracing::warn!(approval = %a.id, channel, account = %acct.id, error = %e, "factory approval request not sent");
                }
            }
        }
    }
    if let Some(email) = send_email_request(state, a).await {
        sent.push(email);
    }
    sent
}

/// The owner's bot to send email from: the node's bot if it has an address, else any of theirs.
fn email_sender(conn: &Connection, owner: &str, bot: Option<&str>) -> Option<String> {
    conn.query_row(
        "SELECT c.agent_id FROM agent_identity_channels c JOIN agents a ON a.id = c.agent_id
         WHERE a.user_id = ?1 AND c.email_address IS NOT NULL AND c.email_address <> ''
         ORDER BY (c.agent_id = ?2) DESC, c.agent_id LIMIT 1",
        params![owner, bot.unwrap_or("")],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

async fn send_email_request(state: &Arc<AppState>, a: &Approval) -> Option<String> {
    let (agent, to, code) = {
        let conn = state.db.connect().ok()?;
        let to = owner_email(&conn, &a.owner)?;
        let agent = email_sender(&conn, &a.owner, a.bot_id.as_deref())?;
        let code = issue_code(&conn, &a.id, "email", Some(&agent), now()).ok()?;
        (agent, to, code)
    };
    let req = crate::agent_email_routes::SendAgentEmailRequest { agent_id: agent.clone(), to: to.clone(), subject: format!("Approve: {}", a.title), text: Some(request_text(a, &code, false)), html: None };
    let res = crate::agent_email_routes::send_owner_notice(state, &a.owner, req).await;
    let conn = state.db.connect().ok()?;
    match res {
        Ok(v) if v.get("status").and_then(Value::as_str) == Some("sent") => {
            record_message(&conn, a, "email", Some(&agent), &to, v.get("messageId").and_then(Value::as_str).unwrap_or(""));
            Some("email".into())
        }
        other => {
            drop_code(&conn, &a.id, "email");
            tracing::warn!(approval = %a.id, result = ?other.map(|v| v.get("status").cloned()), "factory approval email not sent");
            None
        }
    }
}

fn surface_label(s: &str) -> &str {
    match s {
        "app" => "the app",
        "push" => "a push notification",
        "telegram" => "Telegram",
        "slack" => "Slack",
        "sms" => "SMS",
        "email" => "email",
        "engine" => "Gizzi",
        "expiry" => "expiry",
        other => other,
    }
}

/// "Approved by Eoj in Telegram, 14:02 UTC".
pub fn resolution_line(a: &Approval, by_name: &str) -> String {
    let at = a.resolved_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc).format("%H:%M UTC").to_string()).unwrap_or_default();
    let via = surface_label(a.resolved_via.as_deref().unwrap_or("another surface"));
    match a.state.as_str() {
        "approved" => format!("Approved by {by_name} in {via}, {at}"),
        "rejected" => format!("Rejected by {by_name} in {via}, {at}"),
        _ => format!("No longer waiting ({}), {at}", a.state),
    }
}

/// Edit (Telegram) or follow up (Slack, SMS, email) every request message so
/// it says who answered and where.
pub async fn announce_resolution(state: &Arc<AppState>, a: &Approval) {
    let http = http();
    let (rows, name) = {
        let Ok(conn) = state.db.connect() else { return };
        let rows: Vec<(String, Option<String>, String, String)> = conn
            .prepare("SELECT channel, account_id, COALESCE(chat_id, ''), COALESCE(message_id, '') FROM factory_approval_messages WHERE approval_id = ?1 AND state = 'sent'")
            .and_then(|mut q| q.query_map(params![a.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
            .unwrap_or_default();
        let by = a.resolved_by.as_deref().unwrap_or("");
        let name = if by == a.owner { owner_name(&conn, &a.owner) } else if by.is_empty() { "someone".to_string() } else { by.trim_start_matches("user:").to_string() };
        (rows, name)
    };
    let line = resolution_line(a, &name);
    for (channel, account, chat, message) in rows {
        let outcome: Result<(), String> = match channel.as_str() {
            "telegram" => {
                let Some(acct) = account.as_deref().and_then(|id| accounts(&state.db, "telegram", Some(id)).into_iter().next()) else { continue };
                let token = pick(&acct.secret, "botToken");
                let text = format!("Allternit Factory: \"{}\"\n{line}", a.title);
                let edited = http
                    .post_json(HttpReq { url: format!("https://api.telegram.org/bot{token}/editMessageText"), headers: vec![], body: json!({ "chat_id": chat, "message_id": message.parse::<i64>().unwrap_or(0), "text": text }) })
                    .await
                    .ok()
                    .filter(|r| r.body.get("ok").and_then(Value::as_bool) == Some(true));
                match edited {
                    Some(_) => Ok(()),
                    None => telegram_send(&http, &token, &chat, &text, None).await.map(|_| ()),
                }
            }
            "slack" | "sms" => {
                let Some(acct) = account.as_deref().and_then(|id| accounts(&state.db, &channel, Some(id)).into_iter().next()) else { continue };
                match build_transport(&channel, &acct.secret, http.clone()) {
                    None => Err("no transport".into()),
                    Some(tx) => tx
                        .post(&Outbound { workspace: None, channel: chat.clone(), thread: if channel == "sms" { Some(chat.clone()) } else { Some(message.clone()).filter(|m| !m.is_empty()) }, text: format!("\"{}\": {line}", a.title), identity: None })
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("{e:?}")),
                }
            }
            "email" => {
                let Some(agent) = account else { continue };
                let req = crate::agent_email_routes::SendAgentEmailRequest { agent_id: agent, to: chat.clone(), subject: format!("Re: Approve: {}", a.title), text: Some(line.clone()), html: None };
                crate::agent_email_routes::send_owner_notice(state, &a.owner, req).await.map(|_| ()).map_err(|e| format!("{e:?}"))
            }
            _ => continue,
        };
        if let Ok(conn) = state.db.connect() {
            let (st, detail) = match &outcome {
                Ok(()) => ("updated", line.clone()),
                Err(e) => ("update_failed", e.clone()),
            };
            let _ = conn.execute("UPDATE factory_approval_messages SET state = ?3, detail = ?4 WHERE approval_id = ?1 AND channel = ?2", params![a.id, channel, st, detail]);
        }
        if let Err(e) = outcome {
            tracing::warn!(approval = %a.id, channel, error = %e, "factory approval: could not update the request message");
        }
    }
}

// ---------------------------------------------------------------- inbound

/// One inbound channel message that may be an approval answer.
pub struct Answer<'a> {
    pub channel: &'a str,
    pub account: &'a Account,
    pub sender: Option<&'a str>,
    pub text: &'a str,
    pub message_id: &'a str,
    pub forwarded: bool,
}

/// `verify <code>`: pairs the sender as the owner's identity on this account.
fn parse_verify(text: &str) -> Option<String> {
    let w: Vec<&str> = text.split_whitespace().collect();
    (w.len() == 2 && w[0].eq_ignore_ascii_case("verify") && (6..=8).contains(&w[1].len()) && w[1].chars().all(|c| c.is_ascii_alphanumeric())).then(|| w[1].to_uppercase())
}

/// True when `text` is shaped like an approval answer or a verify, i.e. this
/// module owns the message and normal routing must not see it.
pub fn is_answer(text: &str) -> bool {
    parse_reply(text).is_some() || parse_verify(text).is_some()
}

/// Handle an answer. `None` = not ours (route normally). `Some(reply)` =
/// consumed; post `reply` back when non-empty. Wrong senders get no reply.
pub async fn handle_answer(state: &Arc<AppState>, ans: Answer<'_>) -> Option<String> {
    let acct = ans.account;
    let conn = state.db.connect().ok()?;
    if let Some(code) = parse_verify(ans.text) {
        let vid = format!("verify:{}", acct.id);
        let Some(sender) = ans.sender.filter(|s| !s.is_empty()) else {
            refuse(&conn, None, ans.channel, Some(&acct.id), None, Some(ans.message_id), "verify without a sender identity");
            return Some(String::new());
        };
        if ans.forwarded {
            refuse(&conn, None, ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), "forwarded verify message");
            return Some(String::new());
        }
        return match check_code(&conn, &vid, ans.channel, &code, now()) {
            Ok(CodeCheck::Ok) if use_code(&conn, &vid, ans.channel, now()).unwrap_or(false) => {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO factory_owner_identities (account_id, channel, owner, identity, verified_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![acct.id, ans.channel, acct.owner, sender, now().to_rfc3339()],
                );
                Some("Verified. Allternit Factory approvals will come to you here.".into())
            }
            Ok(c) => {
                refuse(&conn, None, ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), &format!("verify: {}", c.reason()));
                Some(String::new())
            }
            Err(_) => Some(String::new()),
        };
    }
    let reply = parse_reply(ans.text)?;
    let sender = ans.sender.unwrap_or("");
    let Some(identity) = owner_identity(&conn, &acct.id, ans.channel, &acct.owner) else {
        refuse(&conn, None, ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), "no verified owner identity on this account");
        return Some(String::new());
    };
    if sender.is_empty() || sender != identity {
        refuse(&conn, None, ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), "sender is not the verified owner");
        return Some(String::new());
    }
    if ans.forwarded {
        refuse(&conn, None, ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), "forwarded message");
        return Some("A forwarded message can't approve. Reply with the code yourself.".into());
    }
    let candidates = fa::pending_for_node(&conn, &acct.owner, &reply.node_id).unwrap_or_default();
    let mut last = CodeCheck::NoCode;
    let mut matched: Option<Approval> = None;
    for c in candidates {
        let bound: Option<Option<String>> = conn.query_row("SELECT account_id FROM factory_approval_codes WHERE approval_id = ?1 AND channel = ?2", params![c.id, ans.channel], |r| r.get(0)).optional().ok().flatten();
        if bound.as_ref().and_then(|b| b.as_deref()) != Some(acct.id.as_str()) {
            continue;
        }
        match check_code(&conn, &c.id, ans.channel, &reply.code, now()) {
            Ok(CodeCheck::Ok) => {
                matched = Some(c);
                break;
            }
            Ok(other) => last = other,
            Err(_) => {}
        }
    }
    let Some(a) = matched else {
        refuse(&conn, None, ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), &format!("no matching code for node {}: {}", reply.node_id, last.reason()));
        return Some(format!("That didn't match a pending approval for {} ({}). You can approve it in the Allternit app.", reply.node_id, last.reason()));
    };
    if a.is_high_risk() {
        refuse(&conn, Some(&a.id), ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), "high-risk approval answered on a channel");
        return Some("This one is high risk: approve it in the Allternit app.".into());
    }
    if !use_code(&conn, &a.id, ans.channel, now()).unwrap_or(false) {
        refuse(&conn, Some(&a.id), ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), "code already used");
        return Some(String::new());
    }
    drop(conn);
    let prov = Provenance { surface: ans.channel.to_string(), account_id: Some(acct.id.clone()), message_id: Some(ans.message_id.to_string()), sender: Some(sender.to_string()) };
    Some(match fa::resolve(state, &a.id, reply.approve, &acct.owner, prov, None).await {
        Ok(done) => format!("{}: \"{}\".", if done.state == "approved" { "Approved" } else { "Rejected" }, done.title),
        Err(ResolveError::AlreadyResolved(cur)) => format!("Already answered: {}.", resolution_line(&cur, "you")),
        Err(e) => {
            let why = format!("{e:?}");
            if let Ok(conn) = state.db.connect() {
                refuse(&conn, Some(&a.id), ans.channel, Some(&acct.id), Some(sender), Some(ans.message_id), &format!("resolve failed: {why}"));
            }
            "That couldn't be recorded. Open the approval in the Allternit app.".into()
        }
    })
}

/// Telegram raw update: inline button presses (`callback_query`) and text
/// answers, with forwarding read from the raw message. `None` = not ours.
pub fn telegram_is_factory(payload: &Value) -> bool {
    if payload.pointer("/callback_query/data").and_then(Value::as_str).is_some_and(|d| d.starts_with("fa:")) {
        return true;
    }
    payload.pointer("/message/text").and_then(Value::as_str).is_some_and(is_answer)
}

fn telegram_forwarded(msg: &Value) -> bool {
    ["forward_origin", "forward_from", "forward_from_chat", "forward_sender_name", "forward_date", "quote", "external_reply"].iter().any(|k| msg.get(*k).is_some())
        || msg.get("is_automatic_forward").and_then(Value::as_bool) == Some(true)
}

pub async fn telegram_update(state: &Arc<AppState>, acct: &Account, payload: &Value) {
    let http = http();
    let token = pick(&acct.secret, "botToken");
    if let Some(cq) = payload.get("callback_query") {
        let data = cq.get("data").and_then(Value::as_str).unwrap_or("");
        let from = cq.pointer("/from/id").map(|v| v.as_i64().map(|n| n.to_string()).unwrap_or_else(|| v.as_str().unwrap_or("").to_string())).unwrap_or_default();
        let cq_id = cq.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let parts: Vec<&str> = data.splitn(4, ':').collect();
        let answer = if let ["fa", id, d @ ("a" | "r"), code] = parts.as_slice() {
            let node = state.db.connect().ok().and_then(|c| fa::get(&c, id).ok().flatten()).map(|a| a.node_id).unwrap_or_default();
            let text = format!("{} {node} {code}", if *d == "a" { "approve" } else { "reject" });
            let mid = cq.pointer("/message/message_id").and_then(Value::as_i64).map(|n| n.to_string()).unwrap_or_default();
            handle_answer(state, Answer { channel: "telegram", account: acct, sender: Some(&from), text: &text, message_id: &mid, forwarded: false }).await.unwrap_or_default()
        } else {
            String::new()
        };
        let _ = http
            .post_json(HttpReq { url: format!("https://api.telegram.org/bot{token}/answerCallbackQuery"), headers: vec![], body: json!({ "callback_query_id": cq_id, "text": answer.chars().take(190).collect::<String>() }) })
            .await;
        return;
    }
    let Some(msg) = payload.get("message") else { return };
    let text = msg.get("text").and_then(Value::as_str).unwrap_or("");
    let from = msg.pointer("/from/id").and_then(Value::as_i64).map(|n| n.to_string());
    let chat = msg.pointer("/chat/id").and_then(Value::as_i64).map(|n| n.to_string()).unwrap_or_default();
    let mid = msg.get("message_id").and_then(Value::as_i64).map(|n| n.to_string()).unwrap_or_default();
    let reply = handle_answer(state, Answer { channel: "telegram", account: acct, sender: from.as_deref(), text, message_id: &mid, forwarded: telegram_forwarded(msg) }).await;
    if let Some(r) = reply.filter(|r| !r.is_empty()) {
        if let Err(e) = telegram_send(&http, &token, &chat, &r, None).await {
            tracing::warn!(error = %e, "factory approval: telegram reply failed");
        }
    }
}

/// Email answer (from `agent_email_routes::receive_inbound_email`, after the
/// loop guards passed). Only the first line of the de-quoted body counts, and
/// a forwarded mail (`Fwd:` / `Fw:` subject) never approves. `true` = consumed.
pub async fn email_answer(state: &Arc<AppState>, owner: &str, agent_id: &str, from: &str, subject: Option<&str>, body: &str, message_id: &str) -> bool {
    let stripped = crate::agent_email_reply::strip_quoted_history(body);
    let first = stripped.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if !is_answer(first) {
        return false;
    }
    let sender = from.rsplit('<').next().unwrap_or(from).trim_end_matches('>').trim().to_lowercase();
    let subj = subject.unwrap_or("").trim_start().to_lowercase();
    let forwarded = subj.starts_with("fwd:") || subj.starts_with("fw:") || first.starts_with('>');
    let Ok(conn) = state.db.connect() else { return true };
    let owner_mail = owner_email(&conn, owner).map(|e| e.to_lowercase());
    if parse_verify(first).is_some() {
        refuse(&conn, None, "email", Some(agent_id), Some(&sender), Some(message_id), "email identities are the account email; nothing to verify");
        return true;
    }
    // Email answers resolve against the owner's account email; codes are bound to the sending bot.
    let acct = Account { id: agent_id.to_string(), owner: owner.to_string(), restricted_bot: None, secret: String::new() };
    if owner_mail.as_deref() != Some(sender.as_str()) {
        refuse(&conn, None, "email", Some(agent_id), Some(&sender), Some(message_id), "sender is not the owner's account email");
        return true;
    }
    let _ = conn.execute(
        "INSERT OR REPLACE INTO factory_owner_identities (account_id, channel, owner, identity, verified_at) VALUES (?1, 'email', ?2, ?3, ?4)",
        params![agent_id, owner, sender, now().to_rfc3339()],
    );
    drop(conn);
    let _ = handle_answer(state, Answer { channel: "email", account: &acct, sender: Some(&sender), text: first, message_id, forwarded }).await;
    true
}

// ---------------------------------------------------------------- identity routes

pub fn identity_router() -> Router<Arc<AppState>> {
    Router::new().route("/factory/approvals/identities", get(list_identities_h).post(start_verify_h).delete(forget_identity_h))
}

async fn list_identities_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let Ok(conn) = state.db.connect() else { return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", "database unavailable", "Try again") };
    let mut out = Vec::new();
    for channel in ACCOUNT_CHANNELS {
        for acct in accounts(&state.db, channel, None).into_iter().filter(|a| a.owner == user.user_id) {
            let identity = owner_identity(&conn, &acct.id, channel, &user.user_id);
            out.push(json!({ "accountId": acct.id, "channel": channel, "verified": identity.is_some(), "identity": identity }));
        }
    }
    let email = owner_email(&conn, &user.user_id);
    let email_from = email_sender(&conn, &user.user_id, None);
    out.push(json!({ "accountId": email_from, "channel": "email", "verified": email.is_some() && email_from.is_some(), "identity": email }));
    Json(json!({ "identities": out })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyBody {
    account_id: String,
}

/// Issue a `verify` code for one of the session owner's accounts. The owner
/// sends `verify <code>` to the bot from their own Slack user / phone /
/// Telegram account within 15 minutes.
async fn start_verify_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<VerifyBody>) -> Response {
    let found = ACCOUNT_CHANNELS.iter().find_map(|ch| accounts(&state.db, ch, Some(&b.account_id)).into_iter().find(|a| a.owner == user.user_id).map(|a| (*ch, a)));
    let Some((channel, acct)) = found else {
        return api_err(StatusCode::NOT_FOUND, "not_found", "no Telegram, Slack or SMS connection with that id on your account", "Connect the channel first");
    };
    let Ok(conn) = state.db.connect() else { return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", "database unavailable", "Try again") };
    let vid = format!("verify:{}", acct.id);
    let code = match issue_code(&conn, &vid, channel, Some(&acct.id), now()) {
        Ok(c) => c,
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", &e.to_string(), "Try again"),
    };
    let expires = now() + Duration::minutes(VERIFY_TTL_MINUTES);
    let _ = conn.execute("UPDATE factory_approval_codes SET expires_at = ?3 WHERE approval_id = ?1 AND channel = ?2", params![vid, channel, expires.to_rfc3339()]);
    Json(json!({ "accountId": acct.id, "channel": channel, "code": code, "send": format!("verify {code}"), "expiresAt": expires.to_rfc3339() })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForgetQuery {
    account_id: String,
}

async fn forget_identity_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<ForgetQuery>) -> Response {
    let Ok(conn) = state.db.connect() else { return api_err(StatusCode::INTERNAL_SERVER_ERROR, "transport", "database unavailable", "Try again") };
    let n = conn.execute("DELETE FROM factory_owner_identities WHERE account_id = ?1 AND owner = ?2", params![q.account_id, user.user_id]).unwrap_or(0);
    Json(json!({ "removed": n })).into_response()
}
