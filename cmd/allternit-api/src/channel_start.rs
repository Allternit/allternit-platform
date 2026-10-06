//! Bot-initiated conversations: a bot starts a chat on a channel instead of
//! only replying to one that already exists, plus the mail thread list.
//!
//! * `GET  /api/v1/channels/:provider/targets?botId=` who/where the bot can start with.
//! * `POST /api/v1/channels/:provider/start`           start it (first message is the bot's outbound).
//! * `GET  /api/v1/mail/threads?botId=&folder=`        inbox | needs_ok | sent for the bot's mailbox.
//!
//! Every provider keeps its platform's honest rules: the rule is checked
//! before any thread exists, so a refused start leaves nothing behind. A
//! started conversation is a normal Allternit thread with a channel binding
//! (same keys the inbound normalizers produce, so replies land in it), and
//! the first message goes out through [`channel_gateway::send`] — the channel
//! tool policy, approval, delivery log and `channel.message.*` events all
//! apply. Email goes through the existing approval-gated mailflare send.
//! Nothing here needs new env: with no connection for the bot the routes
//! answer `not_connected`, and a transport with no credentials answers 503
//! `<provider>_not_configured`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_gateway_routes::{id, now};
use crate::auth::AuthUser;
use crate::channel_gateway::*;
use crate::channel_transports::{accounts, build_transport, pick, Account, HttpSend, ReqwestSend, PROVIDERS};
use crate::db::DbHandle;
use crate::thread_routes::ThreadRuntime;
use crate::AppState;

/// Free-text DM window for WhatsApp Business (Meta's customer service window).
const WA_WINDOW_HOURS: i64 = 24;

pub fn channel_start_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/channels/:provider/targets", get(targets_h))
        .route("/channels/:provider/start", post(start_h).layer(axum::extract::DefaultBodyLimit::max(crate::channel_files::MAX_REQUEST_BYTES)))
        .route("/mail/threads", get(mail_threads_h))
}

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub struct StartError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub extra: Value,
}

impl StartError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        StartError { status, code: code.to_string(), message: message.into(), extra: Value::Null }
    }
    fn with(mut self, extra: Value) -> Self {
        self.extra = extra;
        self
    }
}

impl IntoResponse for StartError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.code, "message": self.message });
        if let (Some(m), Some(x)) = (body.as_object_mut(), self.extra.as_object()) {
            m.extend(x.clone());
        }
        (self.status, Json(body)).into_response()
    }
}

fn bad(code: &str, message: impl Into<String>) -> StartError {
    StartError::new(StatusCode::BAD_REQUEST, code, message)
}

fn conflict(code: &str, message: impl Into<String>) -> StartError {
    StartError::new(StatusCode::CONFLICT, code, message)
}

fn internal(e: impl std::fmt::Display) -> StartError {
    StartError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e.to_string())
}

// ---------------------------------------------------------------- providers + rules

/// `phone` is `sms`; `email` is not a gateway transport but starts the same way.
fn canonical(provider: &str) -> Option<&'static str> {
    match provider {
        "phone" | "sms" => Some("sms"),
        "email" => Some("email"),
        p => PROVIDERS.iter().find(|x| **x == p).copied(),
    }
}

fn place(provider: &str) -> &'static str {
    match provider {
        "telegram" => "Telegram",
        "slack" => "Slack",
        "discord" => "Discord",
        "teams" => "Teams",
        "whatsapp" | "whatsapp-personal" => "WhatsApp",
        "sms" => "SMS",
        "email" => "email",
        _ => "the channel",
    }
}

/// What a start can address — listed kinds (`user`, `group`, `channel`) and the ways to type one in
/// (`username`, `phone`, `email`) — and the platform's rule in one line.
fn rules(provider: &str) -> (&'static [&'static str], &'static str) {
    match provider {
        "telegram" => (&["user", "group", "channel", "username"], "Telegram only lets a bot message a person who has messaged it first. Groups and channels work when the bot is a member."),
        "slack" => (&["user", "channel"], "Posts to a channel the bot has been invited to, or opens a direct message with a Slack user id. A reply in a direct message starts its own conversation."),
        "discord" => (&["channel", "user"], "Posts to a server channel the Allternit Discord app can see, or opens a direct message with a Discord user id. The person must share a server with the Allternit app and allow direct messages from server apps."),
        "teams" => (&["user", "group", "channel"], "Teams only lets a bot message a conversation it already has a reference for (someone installed or messaged it). Proactive sending to anyone else is switched off on the shared app."),
        "whatsapp" => (&["user", "phone"], "WhatsApp only allows free-form messages within 24 hours of the person's last message. Template messages are not supported yet."),
        "whatsapp-personal" => (&["user", "group", "phone"], "Unofficial linked-device number. There is no 24-hour window, but WhatsApp can ban a number that messages people who did not ask for it."),
        "sms" => (&["user", "phone"], "Texts an E.164 number from the bot's number. The cloud enforces consent: the person must have texted or called the number first (or be an allowed contact), and anyone who replied STOP is never texted."),
        "email" => (&["user", "email"], "Composes a new email from the bot's mailbox. It waits for your approval before it is sent."),
        _ => (&[], ""),
    }
}

// ---------------------------------------------------------------- bot + account lookup

fn owns_bot(db: &DbHandle, owner: &str, bot: &str) -> bool {
    db.connect()
        .ok()
        .and_then(|c| c.query_row("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2", params![bot, owner], |_| Ok(())).ok())
        .is_some()
}

fn bot_name(db: &DbHandle, bot: &str) -> String {
    db.connect()
        .ok()
        .and_then(|c| c.query_row("SELECT COALESCE(NULLIF(name, ''), id) FROM agents WHERE id = ?1", params![bot], |r| r.get::<_, String>(0)).ok())
        .unwrap_or_else(|| bot.to_string())
}

/// The owner's connections of `provider` this bot is switched on for.
fn bot_accounts(db: &DbHandle, owner: &str, provider: &str, bot: &str) -> Vec<Account> {
    let Ok(conn) = db.connect() else { return vec![] };
    accounts(db, provider, None)
        .into_iter()
        .filter(|a| a.owner == owner)
        .filter(|a| {
            a.restricted_bot.as_deref() == Some(bot)
                || conn
                    .query_row("SELECT 1 FROM channel_account_bots WHERE account_id = ?1 AND bot_id = ?2 AND owner = ?3", params![a.id, bot, owner], |_| Ok(()))
                    .is_ok()
        })
        .collect()
}

fn not_connected(provider: &str) -> StartError {
    conflict("not_connected", format!("This bot isn't switched on for a {} connection. Connect {0} and switch the bot on in Agent Gateway → Messaging first.", place(provider)))
}

fn not_configured(provider: &str, why: impl Into<String>) -> StartError {
    StartError::new(StatusCode::SERVICE_UNAVAILABLE, &format!("{}_not_configured", provider.replace('-', "_")), why)
}

/// A conversation this owner's connection already has with `channel`.
#[derive(Debug, Clone)]
struct Seen {
    workspace: Option<String>,
    name: Option<String>,
}

fn seen(db: &DbHandle, owner: &str, provider: &str, account: &str, channel: &str) -> Option<Seen> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT b.external_workspace_id, COALESCE(b.channel_name, t.title) FROM channel_conversation_bindings b LEFT JOIN bot_threads t ON t.id = b.thread_id
         WHERE b.owner = ?1 AND b.provider = ?2 AND b.external_channel_id = ?3 AND (b.account_binding_id = ?4 OR b.account_binding_id IS NULL)
         ORDER BY b.updated_at DESC LIMIT 1",
        params![owner, provider, channel, account],
        |r| Ok(Seen { workspace: r.get(0)?, name: r.get(1)? }),
    )
    .optional()
    .ok()
    .flatten()
}

// ---------------------------------------------------------------- start

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub kind: Option<String>,
    pub id: Option<String>,
    pub username: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartBody {
    pub bot_id: String,
    pub target: Target,
    pub text: String,
    #[serde(default)]
    pub attachments: Option<Vec<Value>>,
    /// Email only: the subject (default: the first line of the message).
    #[serde(default)]
    pub subject: Option<String>,
}

/// Where the conversation lives on the platform, in the same shape the
/// provider's inbound normalizer gives the binding (so replies find it).
#[derive(Debug, Clone)]
struct Dest {
    key: String,
    channel: String,
    workspace: Option<String>,
    thread: Option<String>,
    name: String,
    user: Option<String>,
    /// Slack: the conversation key is `slack:<channel>:<root ts>`; the ts is only known after the post.
    rekey_to_ts: bool,
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

fn valid_wa_digits(d: &str) -> bool {
    (7..=15).contains(&d.len())
}

/// Sends email for the start route; production calls the approval-gated mailflare send.
#[async_trait]
pub trait EmailSender: Send + Sync {
    async fn send(&self, owner: &str, bot: &str, to: &str, subject: &str, text: &str, files: &[crate::channel_files::ChannelFile]) -> Result<Value, (StatusCode, Value)>;
}

pub struct MailflareSender(pub Arc<AppState>);

#[async_trait]
impl EmailSender for MailflareSender {
    async fn send(&self, owner: &str, bot: &str, to: &str, subject: &str, text: &str, files: &[crate::channel_files::ChannelFile]) -> Result<Value, (StatusCode, Value)> {
        let req = crate::agent_email_routes::SendAgentEmailRequest { agent_id: bot.into(), to: to.into(), subject: subject.into(), text: Some(text.into()), html: None };
        crate::agent_email_routes::send_email_with_files(&self.0, owner, req, files).await.map_err(|(s, Json(v))| (s, v))
    }
}

pub type TransportFor<'a> = &'a (dyn Fn(&str, &Account) -> Option<Arc<dyn ChannelTransport>> + Send + Sync);

pub struct Deps<'a, R: ThreadRuntime> {
    pub db: &'a DbHandle,
    pub rt: &'a R,
    pub http: Arc<dyn HttpSend>,
    pub transport: TransportFor<'a>,
    pub mail: &'a dyn EmailSender,
}

fn telegram_url(token: &str, method: &str, query: &[(&str, &str)]) -> String {
    let q: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", v.replace('@', "%40"))).collect();
    format!("https://api.telegram.org/bot{token}/{method}?{}", q.join("&"))
}

async fn telegram_dest(db: &DbHandle, http: &dyn HttpSend, owner: &str, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    let token = pick(&acct.secret, "botToken");
    if token.is_empty() {
        return Err(not_configured("telegram", "This Telegram connection has no bot token."));
    }
    let mut chat = nonempty(&t.id);
    if chat.is_none() {
        let Some(u) = nonempty(&t.username).map(|u| u.trim_start_matches('@').to_string()) else { return Err(bad("invalid_target", "Give the Telegram chat id or the @username of a group or channel.")) };
        if !(5..=32).contains(&u.len()) || !u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(bad("invalid_target", "That isn't a Telegram username."));
        }
        let r = http.get_json(&telegram_url(&token, "getChat", &[("chat_id", &format!("@{u}"))])).await.map_err(|e| StartError::new(StatusCode::BAD_GATEWAY, "channel_unreachable", e))?;
        chat = r.body.pointer("/result/id").and_then(Value::as_i64).map(|n| n.to_string());
        if chat.is_none() {
            return Err(conflict("user_never_messaged_bot", "Telegram can't find that username. A person can only be started with after they have messaged the bot; give their chat id."));
        }
    }
    let chat = chat.unwrap_or_default();
    let numeric = chat.trim_start_matches('-');
    if numeric.is_empty() || !numeric.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad("invalid_target", "A Telegram chat id is a number, like 123456789 or -1001234567890."));
    }
    let known = seen(db, owner, "telegram", &acct.id, &chat);
    if known.is_none() {
        if !chat.starts_with('-') {
            return Err(conflict("user_never_messaged_bot", "Telegram bots can only message a person who has messaged the bot first. Ask them to open the bot and press Start."));
        }
        // A group or channel the bot hasn't talked in yet: it must be a member.
        let me = token.split(':').next().unwrap_or_default().to_string();
        let r = http.get_json(&telegram_url(&token, "getChatMember", &[("chat_id", &chat), ("user_id", &me)])).await.map_err(|e| StartError::new(StatusCode::BAD_GATEWAY, "channel_unreachable", e))?;
        let status = r.body.pointer("/result/status").and_then(Value::as_str).unwrap_or_default();
        if r.status != 200 || !matches!(status, "creator" | "administrator" | "member" | "restricted") {
            return Err(conflict("bot_not_in_group", "The bot isn't a member of that Telegram group or channel. Add it there first."));
        }
    }
    Ok(Dest { key: format!("telegram:{chat}"), channel: chat.clone(), workspace: None, thread: None, name: known.and_then(|k| k.name).unwrap_or(chat.clone()), user: None, rekey_to_ts: false })
}

fn slack_dest(db: &DbHandle, owner: &str, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    let Some(target) = nonempty(&t.id) else { return Err(bad("invalid_target", "Give the Slack channel id (C…) or user id (U…).")) };
    if !target.chars().all(|c| c.is_ascii_alphanumeric()) || !target.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return Err(bad("invalid_target", "That isn't a Slack id. Channel ids start with C or G, user ids with U or W."));
    }
    let dm = target.starts_with('U') || target.starts_with('W');
    if t.kind.as_deref() == Some("user") && !(dm || target.starts_with('D')) || matches!(t.kind.as_deref(), Some("channel" | "group")) && dm {
        return Err(bad("invalid_target", "The Slack id doesn't match the target kind."));
    }
    let known = seen(db, owner, "slack", &acct.id, &target);
    Ok(Dest {
        key: format!("slack:{target}:start-{}", uuid::Uuid::new_v4().simple()),
        channel: target.clone(),
        workspace: known.as_ref().and_then(|k| k.workspace.clone()),
        thread: None,
        name: known.and_then(|k| k.name).unwrap_or(target),
        user: None,
        rekey_to_ts: true,
    })
}

/// A direct message: only through the shared Allternit app, whose bot (in the cloud) opens the DM.
/// The DM channel is opened now so replies bind to the channel Discord will deliver them on.
async fn discord_dm_dest(http: &dyn HttpSend, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    if !crate::channel_discord_app::is_app_secret(&acct.secret) {
        return Err(conflict("discord_dm_unavailable", "This Discord connection posts through one channel webhook, which can't reach a person's direct messages. Connect the Allternit Discord app to message people."));
    }
    let user = nonempty(&t.id).filter(|u| (15..=25).contains(&u.len()) && u.chars().all(|c| c.is_ascii_digit())).ok_or_else(|| bad("invalid_target", "Give the Discord user id (a number)."))?;
    let channel = crate::channel_discord_dm::open(http, &acct.secret, &user).await.map_err(dm_error)?;
    Ok(Dest { key: format!("discord:{channel}"), channel, workspace: Some(crate::channel_discord_dm::dm_workspace(&user)), thread: None, name: format!("Discord user {user}"), user: Some(user), rekey_to_ts: false })
}

fn dm_error(e: crate::channel_discord_dm::DmError) -> StartError {
    use crate::channel_discord_dm::{sentence, DmError};
    let (code, message) = sentence(&e);
    match e {
        DmError::NotConfigured => not_configured("discord", message),
        DmError::NotInstalled | DmError::NotInServer | DmError::Closed => conflict(code, message),
        DmError::RateLimited => StartError::new(StatusCode::TOO_MANY_REQUESTS, code, message),
        DmError::Uncertain(_) | DmError::Rejected(_) => StartError::new(StatusCode::BAD_GATEWAY, code, message),
    }
}

fn discord_dest(db: &DbHandle, owner: &str, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    let channel = nonempty(&t.id).filter(|c| c.chars().all(|ch| ch.is_ascii_digit())).ok_or_else(|| bad("invalid_target", "Give the Discord channel id (a number)."))?;
    let known = seen(db, owner, "discord", &acct.id, &channel);
    if !crate::channel_discord_app::is_app_secret(&acct.secret) && known.is_none() {
        return Err(conflict("discord_webhook_fixed_channel", "This Discord connection posts through one channel webhook, so it can only continue conversations Allternit has already seen. Connect the Allternit Discord app to start new ones."));
    }
    Ok(Dest { key: format!("discord:{channel}"), channel: channel.clone(), workspace: known.as_ref().and_then(|k| k.workspace.clone()), thread: None, name: known.and_then(|k| k.name).unwrap_or(channel), user: None, rekey_to_ts: false })
}

fn teams_dest(db: &DbHandle, owner: &str, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    let conv = nonempty(&t.id).ok_or_else(|| bad("invalid_target", "Give the Teams conversation id."))?;
    let Some(known) = seen(db, owner, "teams", &acct.id, &conv) else {
        return Err(conflict("no_conversation_reference", "Teams only lets the bot message a conversation it already has a reference for. Have the person install or message the bot in Teams first."));
    };
    Ok(Dest { key: format!("teams:{conv}"), channel: conv.clone(), workspace: known.workspace.clone(), thread: None, name: known.name.unwrap_or(conv), user: None, rekey_to_ts: false })
}

/// Newest inbound message from `key`'s conversation, if any.
fn last_inbound_at(db: &DbHandle, owner: &str, provider: &str, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let conn = db.connect().ok()?;
    let at: Option<String> = conn
        .query_row(
            "SELECT MAX(l.created_at) FROM channel_message_log l JOIN channel_conversation_bindings b ON b.id = l.binding_id
             WHERE b.owner = ?1 AND b.provider = ?2 AND b.external_conversation_id = ?3 AND l.direction = 'inbound' AND l.kind = 'message'",
            params![owner, provider, key],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    at.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&chrono::Utc))
}

fn whatsapp_dest(db: &DbHandle, owner: &str, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    let wa = digits(&nonempty(&t.phone).or_else(|| nonempty(&t.id)).unwrap_or_default());
    if !valid_wa_digits(&wa) {
        return Err(bad("invalid_target", "Give the person's WhatsApp number with country code, like +14155550123."));
    }
    let pnid = Some(pick(&acct.secret, "phoneNumberId")).filter(|s| !s.is_empty()).ok_or_else(|| not_configured("whatsapp", "This WhatsApp connection has no phone number id."))?;
    let key = format!("whatsapp:{pnid}:{wa}");
    let open = last_inbound_at(db, owner, "whatsapp", &key).is_some_and(|at| chrono::Utc::now() - at < chrono::Duration::hours(WA_WINDOW_HOURS));
    if !open {
        return Err(conflict(
            "outside_24h_window",
            "WhatsApp only allows free-form messages within 24 hours of the person's last message to this number. Starting with a template message isn't supported yet.",
        ));
    }
    Ok(Dest { key, channel: pnid, workspace: None, thread: Some(wa.clone()), name: format!("+{wa}"), user: Some(wa), rekey_to_ts: false })
}

fn whatsapp_personal_dest(t: &Target) -> Result<Dest, StartError> {
    if !crate::channel_whatsapp_personal::enabled() {
        return Err(not_configured("whatsapp-personal", "whatsapp_personal_disabled"));
    }
    let jid = match nonempty(&t.id).filter(|i| i.contains('@')) {
        Some(j) => j,
        None => {
            let d = digits(&nonempty(&t.phone).or_else(|| nonempty(&t.id)).unwrap_or_default());
            if !valid_wa_digits(&d) {
                return Err(bad("invalid_target", "Give a WhatsApp number with country code, or the chat id."));
            }
            format!("{d}@s.whatsapp.net")
        }
    };
    Ok(Dest { key: format!("whatsapp-personal:{jid}"), channel: jid.clone(), workspace: None, thread: None, name: jid, user: None, rekey_to_ts: false })
}

async fn resolve(deps_http: &dyn HttpSend, db: &DbHandle, owner: &str, provider: &str, acct: &Account, t: &Target) -> Result<Dest, StartError> {
    match provider {
        "telegram" => telegram_dest(db, deps_http, owner, acct, t).await,
        "slack" => slack_dest(db, owner, acct, t),
        "discord" if t.kind.as_deref() == Some("user") => discord_dm_dest(deps_http, acct, t).await,
        "discord" => discord_dest(db, owner, acct, t),
        "teams" => teams_dest(db, owner, acct, t),
        "whatsapp" => whatsapp_dest(db, owner, acct, t),
        "whatsapp-personal" => whatsapp_personal_dest(t),
        _ => Err(StartError::new(StatusCode::NOT_FOUND, "unknown_provider", format!("No channel called {provider}."))),
    }
}

/// A platform's refusal, in the codes the UI explains.
fn refusal(provider: &str, msg: &str) -> StartError {
    let m = msg.to_lowercase();
    let has = |s: &str| m.contains(s);
    if has("outside_24h_window") {
        conflict("outside_24h_window", "WhatsApp only allows free-form messages within 24 hours of the person's last message. Template messages aren't supported yet.")
    } else if has("not_in_channel") {
        conflict("bot_not_in_channel", "The bot isn't in that Slack channel. Invite it there first.")
    } else if has("channel_not_found") {
        StartError::new(StatusCode::NOT_FOUND, "channel_not_found", "Slack can't find that channel or user.")
    } else if has("channel_not_in_guild") {
        conflict("channel_not_in_guild", "That channel isn't in the Discord server this connection is installed in.")
    } else if provider == "teams" && (has("404") || has("conversation reference")) {
        conflict("no_conversation_reference", "Teams has no conversation reference for that chat. Have the person install or message the bot first.")
    } else if has("can't initiate conversation") || has("bot can't initiate") {
        conflict("user_never_messaged_bot", "Telegram bots can only message a person who has messaged the bot first.")
    } else if has("attachments_unsupported") {
        bad("attachments_unsupported", format!("Attachments can't be sent in a {} conversation yet.", place(provider)))
    } else if has("discord_dm_closed") {
        conflict("discord_dm_closed", crate::channel_discord_dm::sentence(&crate::channel_discord_dm::DmError::Closed).1)
    } else if has("discord_user_not_in_server") {
        conflict("discord_user_not_in_server", crate::channel_discord_dm::sentence(&crate::channel_discord_dm::DmError::NotInServer).1)
    } else if has("blocked by the user") {
        conflict("user_blocked_bot", "That person has blocked the bot.")
    } else if has("configured") || has("not set") || has("not linked") || has("not set up") {
        not_configured(provider, msg)
    } else {
        StartError::new(StatusCode::BAD_GATEWAY, "channel_rejected", msg)
    }
}

fn thread_of_session(db: &DbHandle, session: &str) -> Result<String, StartError> {
    db.connect().map_err(internal)?.query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![session], |r| r.get(0)).map_err(internal)
}

/// The thread (and binding) for `dest`: the one already bound to this
/// conversation, else a new thread on `bot` with a fresh binding.
async fn thread_for<R: ThreadRuntime>(deps: &Deps<'_, R>, owner: &str, bot: &str, provider: &str, acct: &Account, d: &Dest, text: &str) -> Result<(BindingRow, bool), StartError> {
    if let Some(b) = find_binding(deps.db, provider, &d.key).filter(|b| b.owner == owner) {
        let thread_bot: Option<String> = deps.db.connect().map_err(internal)?.query_row("SELECT bot_id FROM bot_threads WHERE id = ?1", params![b.thread_id], |r| r.get(0)).ok();
        if thread_bot.is_some_and(|t| t != bot) {
            return Err(conflict("conversation_belongs_to_other_bot", format!("Another of your bots already holds this {} conversation.", place(provider))));
        }
        return Ok((b, true));
    }
    let title: String = format!("{} · {}", place(provider), d.name).chars().take(80).collect();
    let objective = format!("You started this {} conversation with {}. Your first message to them: {text}", place(provider), d.name);
    let session = crate::thread_routes::channel_thread(deps.db, deps.rt, bot, provider, &d.key, &title, &objective).await.map_err(internal)?;
    let thread_id = thread_of_session(deps.db, &session)?;
    let ev = Inbound {
        kind: InboundKind::Message,
        workspace: d.workspace.clone(),
        channel: d.channel.clone(),
        conversation: d.key.clone(),
        thread: d.thread.clone(),
        remote_id: String::new(),
        message_id: String::new(),
        text: None,
        user: d.user.clone(),
        reaction: None,
        added: None,
        cursor: None,
        own: true,
    };
    let b = ensure_binding_on(deps.db, owner, &thread_id, provider, &ev, Some(&acct.id)).map_err(internal)?;
    if let Ok(conn) = deps.db.connect() {
        let _ = conn.execute("UPDATE channel_conversation_bindings SET channel_name = COALESCE(channel_name, ?2) WHERE id = ?1", params![b.id, d.name]);
    }
    Ok((b, false))
}

fn conversation_id(key: &str) -> String {
    key.split_once(':').map(|(_, c)| c.to_string()).unwrap_or_else(|| key.to_string())
}

/// Start a conversation. `Ok((status, body))`: 200 sent, 202 waiting (email
/// approval, unconfirmed delivery); `Err` is a definite refusal.
pub async fn start_conversation<R: ThreadRuntime>(deps: &Deps<'_, R>, owner: &str, provider_raw: &str, body: StartBody) -> Result<(StatusCode, Value), StartError> {
    let provider = canonical(provider_raw).ok_or_else(|| StartError::new(StatusCode::NOT_FOUND, "unknown_provider", format!("No channel called {provider_raw}.")))?;
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(bad("text_required", "Write the first message."));
    }
    // Checked before any thread exists, so a refused file leaves nothing behind.
    let files = crate::channel_files::parse(provider, body.attachments.as_deref().unwrap_or_default()).map_err(|e| file_error(provider, e))?;
    if !owns_bot(deps.db, owner, &body.bot_id) {
        return Err(StartError::new(StatusCode::NOT_FOUND, "bot_not_found", "That bot doesn't exist."));
    }
    let bot = body.bot_id.clone();
    if provider == "email" {
        return start_email(deps, owner, &bot, &body.target, &text, body.subject.as_deref(), &files).await;
    }
    if provider == "sms" {
        return start_sms(deps, owner, &bot, &body.target, &text).await;
    }
    let acct = bot_accounts(deps.db, owner, provider, &bot).into_iter().next().ok_or_else(|| not_connected(provider))?;
    let dest = resolve(deps.http.as_ref(), deps.db, owner, provider, &acct, &body.target).await?;
    let tx = (deps.transport)(provider, &acct).ok_or_else(|| not_configured(provider, format!("{} isn't set up on this runtime.", place(provider))))?;
    let (binding, existing) = thread_for(deps, owner, &bot, provider, &acct, &dest, &text).await?;
    let req = SendReq { text: text.clone(), correlation_id: Some(id("start")), files, ..Default::default() };
    let outcome = send(deps.db, tx.as_ref(), owner, &binding.thread_id, &req).await.map_err(internal)?;
    let reply = |status: StatusCode, state: &str, remote: Option<String>, conv: &str| {
        (status, json!({ "threadId": binding.thread_id, "conversationId": conversation_id(conv), "bindingId": binding.id, "provider": provider, "state": state, "remoteId": remote, "existing": existing }))
    };
    match outcome {
        SendOutcome::Sent { remote_id, .. } | SendOutcome::Replay { state: _, remote_id: Some(remote_id) } => {
            let mut conv = binding.conversation.clone();
            if dest.rekey_to_ts && !existing {
                // Replies thread under this post: bind the conversation to its ts.
                conv = format!("slack:{}:{remote_id}", dest.channel);
                if let Ok(c) = deps.db.connect() {
                    let _ = c.execute(
                        "UPDATE channel_conversation_bindings SET external_conversation_id = ?2, external_thread_id = ?3, updated_at = ?4 WHERE id = ?1",
                        params![binding.id, conv, remote_id, now()],
                    );
                }
            }
            Ok(reply(StatusCode::OK, "sent", Some(remote_id), &conv))
        }
        SendOutcome::Replay { state, remote_id: None } => Ok(reply(StatusCode::ACCEPTED, &state, None, &binding.conversation)),
        SendOutcome::Unconfirmed { .. } => Ok(reply(StatusCode::ACCEPTED, "unconfirmed", None, &binding.conversation)),
        SendOutcome::ApprovalRequired { approval_id } => Err(StartError::new(StatusCode::PRECONDITION_REQUIRED, "approval_required", "This message needs your approval before it is sent.").with(json!({ "threadId": binding.thread_id, "approvalId": approval_id }))),
        SendOutcome::Denied(why) => Err(StartError::new(StatusCode::FORBIDDEN, "channel_policy", why)),
        SendOutcome::ReadOnly => Err(StartError::new(StatusCode::FORBIDDEN, "read_only", "That conversation is read-only.")),
        SendOutcome::NoBinding => Err(internal("the new conversation has no binding")),
        SendOutcome::Rejected(why) => Err(refusal(provider, &why).with(json!({ "threadId": binding.thread_id }))),
    }
}

fn file_error(provider: &str, e: crate::channel_files::FileError) -> StartError {
    let status = if e.code() == "attachment_too_large" { StatusCode::PAYLOAD_TOO_LARGE } else { StatusCode::BAD_REQUEST };
    StartError::new(status, e.code(), e.sentence(place(provider)))
}

fn out_error(e: crate::phone_outbound::OutError) -> StartError {
    use crate::phone_outbound::OutError;
    let message = e.sentence();
    match e {
        OutError::NoConsent(_) | OutError::NoTextConsent(_) => StartError::new(StatusCode::FORBIDDEN, "no_consent", message),
        OutError::CallsUnavailable => StartError::new(StatusCode::SERVICE_UNAVAILABLE, "calls_unavailable", message),
        OutError::OptedOut(_) => StartError::new(StatusCode::FORBIDDEN, "recipient_opted_out", message),
        OutError::NotActive => StartError::new(StatusCode::FORBIDDEN, "sms_not_active", message),
        OutError::DailyLimit => StartError::new(StatusCode::TOO_MANY_REQUESTS, "daily_limit", message),
        OutError::BadRequest(_) => bad("invalid_target", message),
        OutError::NotFound(_) => not_connected("sms"),
        OutError::Failed(_) => StartError::new(StatusCode::BAD_GATEWAY, "channel_rejected", message),
    }
}

/// Texts go through the outbound-phone path (`POST /phone/text`'s code): the bot's number, the
/// cloud's consent gate and STOP list, and the `phone:<number>:<caller>` thread that calls share.
async fn start_sms<R: ThreadRuntime>(deps: &Deps<'_, R>, owner: &str, bot: &str, t: &Target, text: &str) -> Result<(StatusCode, Value), StartError> {
    let to = nonempty(&t.phone).or_else(|| nonempty(&t.id)).unwrap_or_default().replace([' ', '-', '(', ')'], "");
    if !crate::channel_phone::is_e164(&to) {
        return Err(bad("invalid_target", "Give the number in E.164 form, like +14155550123."));
    }
    let n = crate::phone_outbound::pick_number(deps.db, owner, Some(bot), None).map_err(out_error)?;
    let key = crate::channel_phone::phone_key(&n.e164, &to);
    let existing = find_binding(deps.db, "sms", &key).is_some_and(|b| b.owner == owner);
    let (thread_id, message_id) = crate::phone_outbound::text(deps.db, deps.rt, deps.http.clone(), &n, &to, text).await.map_err(out_error)?;
    let binding = binding_for_thread(deps.db, owner, &thread_id).map(|b| b.id);
    Ok((StatusCode::OK, json!({ "threadId": thread_id, "conversationId": conversation_id(&key), "bindingId": binding, "provider": "sms", "state": "sent", "remoteId": message_id, "existing": existing })))
}

fn valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    s.len() <= 254 && !s.chars().any(|c| c.is_whitespace() || c.is_control()) && !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

async fn start_email<R: ThreadRuntime>(deps: &Deps<'_, R>, owner: &str, bot: &str, t: &Target, text: &str, subject: Option<&str>, files: &[crate::channel_files::ChannelFile]) -> Result<(StatusCode, Value), StartError> {
    let to = nonempty(&t.email).or_else(|| nonempty(&t.id)).unwrap_or_default();
    if !valid_email(&to) {
        return Err(bad("invalid_target", "Give the email address to write to."));
    }
    let channel = {
        let conn = deps.db.connect().map_err(internal)?;
        crate::agent_email_routes::lookup_email_channel(&conn, bot).map_err(internal)?
    };
    let Some(channel) = channel else { return Err(not_connected("email")) };
    if !channel.send_enabled {
        return Err(StartError::new(StatusCode::FORBIDDEN, "email_send_disabled", "Outbound email is switched off for this bot."));
    }
    let subject: String = subject
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Message").trim().chars().take(78).collect());
    // Send first: a refused send leaves no thread behind.
    let sent = deps.mail.send(owner, bot, &to, &subject, text, files).await.map_err(|(status, v)| {
        StartError::new(status, v["error"].as_str().unwrap_or("email_failed"), v["message"].as_str().unwrap_or("The email could not be sent."))
    })?;
    let state = sent["status"].as_str().unwrap_or("sent").to_string();
    let key = format!("email:{}:{}", to.to_lowercase(), crate::thread_routes::conversation_subject(&subject));
    let objective = format!("You started this email conversation with {to}. Your first message: {text}");
    let session = crate::thread_routes::channel_thread(deps.db, deps.rt, bot, "email", &key, &subject, &objective).await.map_err(internal)?;
    let thread_id = thread_of_session(deps.db, &session)?;
    let pending = state == "pending_approval";
    let outbound = sent["id"].as_str().unwrap_or_default();
    let kept = crate::channel_attachments::sent_entries_prod(files).await;
    crate::gateway_runner::led(
        deps.db,
        bot,
        &thread_id,
        None,
        if pending { "channel.message.pending" } else { "channel.message.sent" },
        ("bot", bot),
        json!({ "provider": "email", "to": to, "subject": subject, "text": text, "attachments": crate::channel_files::names(files), "files": kept, "state": if pending { "awaiting_approval" } else { "confirmed" }, "outboundId": outbound, "approvalThread": sent["thread"], "messageId": sent["messageId"] }),
        Some(format!("email:start:{outbound}")),
    );
    Ok((
        if pending { StatusCode::ACCEPTED } else { StatusCode::OK },
        json!({ "threadId": thread_id, "conversationId": conversation_id(&key), "provider": "email", "state": state, "remoteId": sent["messageId"], "approvalThread": sent["thread"], "existing": false }),
    ))
}

// ---------------------------------------------------------------- targets

fn target_kind(provider: &str, channel: &str) -> &'static str {
    match provider {
        "telegram" if channel.starts_with('-') => "group",
        "slack" if channel.starts_with('C') || channel.starts_with('G') => "channel",
        "discord" => "channel",
        "teams" if channel.contains("@thread") => "group",
        "whatsapp-personal" if channel.ends_with("@g.us") => "group",
        _ => "user",
    }
}

/// People and places the bot already has a conversation with (newest first).
fn known_targets(db: &DbHandle, owner: &str, provider: &str, accts: &[Account]) -> Vec<Value> {
    let Ok(conn) = db.connect() else { return vec![] };
    let ids: Vec<&str> = accts.iter().map(|a| a.id.as_str()).collect();
    let mut q = match conn.prepare(
        "SELECT b.external_channel_id, b.external_thread_id, b.external_conversation_id, COALESCE(b.channel_name, t.title), b.updated_at, b.account_binding_id
         FROM channel_conversation_bindings b LEFT JOIN bot_threads t ON t.id = b.thread_id
         WHERE b.owner = ?1 AND b.provider = ?2 ORDER BY b.updated_at DESC LIMIT 300",
    ) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let rows: Vec<(Option<String>, Option<String>, String, Option<String>, Option<String>, Option<String>)> = q
        .query_map(params![owner, provider], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map(|rs| rs.filter_map(Result::ok).collect())
        .unwrap_or_default();
    let mut out: BTreeMap<String, (usize, Value)> = BTreeMap::new();
    for (i, (channel, thread, conv, name, updated, account)) in rows.into_iter().enumerate() {
        if account.as_deref().is_some_and(|a| !ids.contains(&a)) {
            continue;
        }
        // The person, where the platform keys a chat by (channel, person).
        let (id, label) = match provider {
            "whatsapp" => match thread {
                Some(wa) => (wa.clone(), format!("+{wa}")),
                None => continue,
            },
            "sms" => match thread {
                Some(p) => (p.clone(), p),
                None => continue,
            },
            _ => match channel {
                Some(c) => (c, String::new()),
                None => continue,
            },
        };
        let _ = &conv;
        let kind = target_kind(provider, &id);
        let name = name.filter(|n| !n.is_empty()).unwrap_or(if label.is_empty() { id.clone() } else { label });
        out.entry(id.clone()).or_insert((i, json!({ "kind": kind, "id": id, "name": name, "sub": updated })));
    }
    let mut v: Vec<(usize, Value)> = out.into_values().collect();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, t)| t).collect()
}

/// Channels the bot has joined, from Slack (needs a runtime-held bot token).
async fn slack_joined_channels(http: &dyn HttpSend, token: &str) -> Vec<Value> {
    let form = vec![
        ("token".to_string(), token.to_string()),
        ("types".to_string(), "public_channel,private_channel".to_string()),
        ("exclude_archived".to_string(), "true".to_string()),
        ("limit".to_string(), "200".to_string()),
    ];
    let Ok(r) = http.post_form("https://slack.com/api/conversations.list", form).await else { return vec![] };
    if r.body["ok"].as_bool() != Some(true) {
        return vec![];
    }
    r.body["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["is_member"].as_bool() == Some(true))
        .filter_map(|c| Some(json!({ "kind": "channel", "id": c["id"].as_str()?, "name": format!("#{}", c["name"].as_str().unwrap_or_default()), "sub": "member" })))
        .collect()
}

fn email_targets(db: &DbHandle, bot: &str) -> Vec<Value> {
    let Ok(conn) = db.connect() else { return vec![] };
    let mut out: Vec<Value> = vec![];
    let mut seen_addr = std::collections::HashSet::new();
    for sql in [
        "SELECT from_address, MAX(created_at) FROM agent_email_inbound WHERE agent_id = ?1 AND guard_reason IS NULL AND from_address IS NOT NULL GROUP BY lower(from_address) ORDER BY 2 DESC LIMIT 50",
        "SELECT to_address, MAX(created_at) FROM agent_email_outbound WHERE agent_id = ?1 GROUP BY lower(to_address) ORDER BY 2 DESC LIMIT 50",
    ] {
        let Ok(mut q) = conn.prepare(sql) else { continue };
        let rows: Vec<(String, Option<String>)> = q.query_map(params![bot], |r| Ok((r.get(0)?, r.get(1)?))).map(|rs| rs.filter_map(Result::ok).collect()).unwrap_or_default();
        for (addr, at) in rows {
            if seen_addr.insert(addr.to_lowercase()) {
                out.push(json!({ "kind": "user", "id": addr, "name": addr, "sub": at.map(|a| rfc3339(&a)) }));
            }
        }
    }
    out
}

pub async fn list_targets(db: &DbHandle, http: &dyn HttpSend, owner: &str, provider_raw: &str, bot: &str, slack_token: Option<&str>) -> Result<Value, StartError> {
    let provider = canonical(provider_raw).ok_or_else(|| StartError::new(StatusCode::NOT_FOUND, "unknown_provider", format!("No channel called {provider_raw}.")))?;
    if !owns_bot(db, owner, bot) {
        return Err(StartError::new(StatusCode::NOT_FOUND, "bot_not_found", "That bot doesn't exist."));
    }
    let (kinds, rule) = rules(provider);
    if provider == "email" {
        let connected = db.connect().ok().and_then(|c| crate::agent_email_routes::lookup_email_channel(&c, bot).ok().flatten()).is_some();
        let notes = if connected { rule.to_string() } else { format!("This bot has no mailbox yet. {rule}") };
        return Ok(json!({ "targets": email_targets(db, bot), "canStartWith": kinds, "connected": connected, "notes": notes }));
    }
    let accts = bot_accounts(db, owner, provider, bot);
    if accts.is_empty() {
        return Ok(json!({ "targets": [], "canStartWith": kinds, "connected": false, "notes": format!("This bot isn't switched on for a {} connection yet.", place(provider)) }));
    }
    let mut targets = known_targets(db, owner, provider, &accts);
    let mut notes = rule.to_string();
    if provider == "slack" {
        match slack_token.filter(|t| !t.is_empty()) {
            Some(token) => {
                let have: std::collections::HashSet<String> = targets.iter().filter_map(|t| t["id"].as_str().map(str::to_string)).collect();
                targets.extend(slack_joined_channels(http, token).await.into_iter().filter(|t| t["id"].as_str().is_some_and(|i| !have.contains(i))));
            }
            None => notes.push_str(" Channels the bot has joined are listed once someone has posted in them; you can also enter a channel id."),
        }
    }
    if provider == "discord" || provider == "teams" {
        notes.push_str(" Only places Allternit has already seen are listed.");
    }
    if provider == "whatsapp" {
        // Only people inside the 24-hour window can be started with.
        let key_prefix = accts.iter().map(|a| pick(&a.secret, "phoneNumberId")).find(|p| !p.is_empty());
        if let Some(pnid) = key_prefix {
            targets.retain(|t| t["id"].as_str().is_some_and(|wa| last_inbound_at(db, owner, "whatsapp", &format!("whatsapp:{pnid}:{wa}")).is_some_and(|at| chrono::Utc::now() - at < chrono::Duration::hours(WA_WINDOW_HOURS))));
        }
    }
    Ok(json!({ "targets": targets, "canStartWith": kinds, "connected": true, "notes": notes }))
}

// ---------------------------------------------------------------- mail threads

/// sqlite `CURRENT_TIMESTAMP` ("2026-10-02 12:00:00", UTC) as RFC 3339.
fn rfc3339(s: &str) -> String {
    if s.len() == 19 && s.as_bytes().get(10) == Some(&b' ') {
        format!("{}T{}Z", &s[..10], &s[11..])
    } else {
        s.to_string()
    }
}

fn thread_for_key(conn: &rusqlite::Connection, bot: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM bot_threads WHERE bot_id = ?1 AND json_extract(origin, '$.channelKey') = ?2 ORDER BY updated_at DESC LIMIT 1",
        params![bot, key],
        |r| r.get(0),
    )
    .ok()
}

/// The bot's mail as threads. `inbox`: mail received (one per sender+subject,
/// unread while the bot hasn't answered). `needs_ok`: outbound waiting for
/// your approval (the id is the review thread `POST /api/factory/mail/decide`
/// takes). `sent`: mail that went out.
pub fn mail_threads(db: &DbHandle, owner: &str, bot: &str, folder: &str) -> Result<Value, StartError> {
    if !owns_bot(db, owner, bot) {
        return Err(StartError::new(StatusCode::NOT_FOUND, "bot_not_found", "That bot doesn't exist."));
    }
    let conn = db.connect().map_err(internal)?;
    let own_address = crate::agent_email_routes::lookup_email_channel(&conn, bot).ok().flatten().map(|c| c.address).unwrap_or_default();
    let mut threads: Vec<Value> = vec![];
    match folder {
        "inbox" => {
            let mut q = conn
                .prepare(
                    "SELECT id, from_address, subject, snippet, created_at, reply_status FROM agent_email_inbound
                     WHERE agent_id = ?1 AND guard_reason IS NULL ORDER BY created_at DESC, rowid DESC LIMIT 500",
                )
                .map_err(internal)?;
            let rows: Vec<(String, String, Option<String>, Option<String>, String, Option<String>)> = q
                .query_map(params![bot], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default(), r.get(5)?)))
                .map_err(internal)?
                .filter_map(Result::ok)
                .collect();
            let mut done = std::collections::HashSet::new();
            for (inbound, from, subject, preview, at, reply_status) in rows {
                let subj = subject.unwrap_or_else(|| "(no subject)".into());
                let key = format!("email:{}:{}", from.to_lowercase(), crate::thread_routes::conversation_subject(&subj));
                if !done.insert(key.clone()) {
                    continue;
                }
                let answered = matches!(reply_status.as_deref(), Some("sent" | "pending_approval" | "approved"));
                threads.push(json!({ "id": thread_for_key(&conn, bot, &key).unwrap_or(inbound), "subject": subj, "from": from, "preview": preview.unwrap_or_default(), "at": rfc3339(&at), "unread": !answered }));
            }
        }
        "needs_ok" | "sent" => {
            let status = if folder == "needs_ok" { "pending_approval" } else { "sent" };
            let mut q = conn
                .prepare("SELECT thread_id, to_address, subject, snippet, created_at FROM agent_email_outbound WHERE agent_id = ?1 AND user_id = ?2 AND status = ?3 ORDER BY created_at DESC, rowid DESC LIMIT 500")
                .map_err(internal)?;
            let rows: Vec<(String, String, Option<String>, Option<String>, String)> = q
                .query_map(params![bot, owner, status], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default())))
                .map_err(internal)?
                .filter_map(Result::ok)
                .collect();
            for (thread, to, subject, preview, at) in rows {
                let mut t = json!({ "id": thread, "subject": subject.unwrap_or_else(|| "(no subject)".into()), "from": own_address, "to": to, "preview": preview.unwrap_or_default(), "at": rfc3339(&at), "unread": folder == "needs_ok" });
                if folder == "needs_ok" {
                    t["needsApproval"] = json!(true);
                }
                threads.push(t);
            }
        }
        _ => return Err(bad("invalid_folder", "folder must be inbox, needs_ok or sent")),
    }
    Ok(json!({ "threads": threads }))
}

// ---------------------------------------------------------------- HTTP

async fn targets_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(provider): Path<String>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(bot) = q.get("botId").filter(|b| !b.is_empty()) else { return bad("bot_required", "botId is required").into_response() };
    // A shared Slack app's token lives in the cloud; only a legacy install has one here.
    let slack_token = if provider == "slack" && !bot_accounts(&state.db, &user.user_id, "slack", bot).iter().any(|a| crate::channel_slack_app::is_shared_secret(&a.secret)) {
        crate::config::AppConfig::load().slack_bot_token()
    } else {
        None
    };
    match list_targets(&state.db, &ReqwestSend, &user.user_id, &provider, bot, slack_token.as_deref()).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Start with the production transports, mailflare and runtime.
async fn start_in_production(state: &Arc<AppState>, owner: &str, provider: &str, body: StartBody) -> Result<(StatusCode, Value), StartError> {
    let rt = crate::thread_routes::GizziRuntime { db: state.db.clone() };
    let http: Arc<dyn HttpSend> = Arc::new(ReqwestSend);
    let h = http.clone();
    let transport = move |p: &str, a: &Account| build_transport(p, &a.secret, h.clone());
    let mail = MailflareSender(state.clone());
    let deps = Deps { db: &state.db, rt: &rt, http, transport: &transport, mail: &mail };
    start_conversation(&deps, owner, provider, body).await
}

async fn start_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(provider): Path<String>, Json(body): Json<StartBody>) -> Response {
    match start_in_production(&state, &user.user_id, &provider, body).await {
        Ok((status, v)) => (status, Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Lets a vendor bot (the MCP connector's `post_message`) start a conversation through the same
/// path as the app, so every platform rule, policy and approval applies. Failures come back as
/// a plain sentence the vendor's agent can read, not a code.
pub struct VendorStarter(pub Arc<AppState>);

#[async_trait]
impl crate::mcp_vendor_bots::ChannelStarter for VendorStarter {
    async fn start(&self, owner: &str, bot_id: &str, provider: &str, target: &Value, text: &str) -> Result<Value, String> {
        let target: Target = serde_json::from_value(target.clone()).map_err(|_| "The target should look like { kind, id } (or username, phone, email).".to_string())?;
        let body = StartBody { bot_id: bot_id.to_string(), target, text: text.to_string(), attachments: None, subject: None };
        vendor_result(start_in_production(&self.0, owner, provider, body).await)
    }
}

/// What a vendor bot is told about a start: ids and state on success, one plain sentence on refusal.
fn vendor_result(r: Result<(StatusCode, Value), StartError>) -> Result<Value, String> {
    match r {
        Ok((status, v)) => Ok(json!({
            "threadId": v["threadId"],
            "provider": v["provider"],
            "state": v["state"],
            "waitingForApproval": status == StatusCode::ACCEPTED && v["state"] == "pending_approval",
        })),
        Err(e) if e.code == "approval_required" => Err("That needs the owner's approval in Allternit first.".into()),
        Err(e) => Err(e.message),
    }
}

/// Register [`VendorStarter`] with the vendor connector. Called once at boot.
pub fn register_vendor_starter(state: &Arc<AppState>) {
    crate::mcp_vendor_bots::register_channel_starter(Arc::new(VendorStarter(state.clone())));
}

async fn mail_threads_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(bot) = q.get("botId").filter(|b| !b.is_empty()) else { return bad("bot_required", "botId is required").into_response() };
    match mail_threads(&state.db, &user.user_id, bot, q.get("folder").map(String::as_str).unwrap_or("inbox")) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::{HttpReq, HttpResp};
    use std::sync::Mutex;

    struct Rt;
    impl ThreadRuntime for Rt {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, id: &str) -> Result<String, String> {
            Ok(format!("sess-{id}"))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, _s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Err("no".into())
        }
    }

    /// Records every post; answers with `result`.
    struct FakeTx {
        provider: &'static str,
        files: Mutex<Vec<Vec<String>>>,
        sent: Mutex<Vec<Outbound>>,
        result: Mutex<Result<Receipt, PostError>>,
    }
    impl FakeTx {
        fn new(provider: &'static str) -> Arc<Self> {
            Arc::new(FakeTx { provider, files: Mutex::new(vec![]), sent: Mutex::new(vec![]), result: Mutex::new(Ok(Receipt { remote_id: "1700000000.000100".into(), relayed: false })) })
        }
        fn answer(&self, r: Result<Receipt, PostError>) {
            *self.result.lock().unwrap() = r;
        }
        fn count(&self) -> usize {
            self.sent.lock().unwrap().len()
        }
    }
    #[async_trait]
    impl ChannelTransport for FakeTx {
        fn provider(&self) -> &'static str {
            self.provider
        }
        fn verify(&self, _s: &str, _h: &axum::http::HeaderMap, _b: &[u8]) -> Result<(), String> {
            Err("fake".into())
        }
        fn normalize(&self, _p: &Value) -> Vec<Inbound> {
            vec![]
        }
        fn identity(&self, requested: Option<&str>) -> Identity {
            Identity { id: requested.map(str::to_string), exact: true }
        }
        async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
            self.sent.lock().unwrap().push(out.clone());
            self.result.lock().unwrap().clone()
        }
        async fn post_files(&self, out: &Outbound, files: &[crate::channel_files::ChannelFile]) -> Result<Receipt, PostError> {
            self.files.lock().unwrap().push(crate::channel_files::names(files));
            self.post(out).await
        }
    }

    /// GETs answer by URL fragment; form posts record and answer `form`.
    #[derive(Default)]
    struct FakeHttp {
        gets: Mutex<Vec<String>>,
        get_replies: Mutex<Vec<(String, u16, Value)>>,
        form: Mutex<Option<Value>>,
        posts: Mutex<Vec<HttpReq>>,
        post_reply: Mutex<Option<(u16, Value)>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.posts.lock().unwrap().push(req);
            let (status, body) = self.post_reply.lock().unwrap().clone().unwrap_or((200, json!({ "messageId": "sms-1", "status": "queued" })));
            Ok(HttpResp { status, body })
        }
        async fn get_json(&self, url: &str) -> Result<HttpResp, String> {
            self.gets.lock().unwrap().push(url.to_string());
            let hit = self.get_replies.lock().unwrap().iter().find(|(frag, _, _)| url.contains(frag.as_str())).cloned();
            Ok(hit.map(|(_, status, body)| HttpResp { status, body }).unwrap_or(HttpResp { status: 404, body: json!({ "ok": false }) }))
        }
        async fn post_form(&self, _url: &str, _form: Vec<(String, String)>) -> Result<HttpResp, String> {
            Ok(HttpResp { status: 200, body: self.form.lock().unwrap().clone().unwrap_or(Value::Null) })
        }
    }

    struct FakeMail {
        files: Mutex<Vec<Vec<String>>>,
        sent: Mutex<Vec<(String, String, String)>>,
        reply: Mutex<Result<Value, (StatusCode, Value)>>,
    }
    impl FakeMail {
        fn new() -> Self {
            FakeMail { files: Mutex::new(vec![]), sent: Mutex::new(vec![]), reply: Mutex::new(Ok(json!({ "status": "pending_approval", "id": "out-1", "thread": "mail:email-out-out-1", "messageId": "msg-1" }))) }
        }
    }
    #[async_trait]
    impl EmailSender for FakeMail {
        async fn send(&self, _owner: &str, _bot: &str, to: &str, subject: &str, text: &str, files: &[crate::channel_files::ChannelFile]) -> Result<Value, (StatusCode, Value)> {
            self.files.lock().unwrap().push(crate::channel_files::names(files));
            self.sent.lock().unwrap().push((to.into(), subject.into(), text.into()));
            self.reply.lock().unwrap().clone()
        }
    }

    async fn state(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-cs-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','Scout','m','p',1,'{}')", []).unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-9','user-b','Other','m','p',1,'{}')", []).unwrap();
        st
    }

    fn account(st: &Arc<AppState>, id: &str, provider: &str, secret: Value) {
        let c = st.db.connect().unwrap();
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, restricted_bot_id, state, created_at, updated_at) VALUES (?1,'user-a',?2,'api_key',?3,NULL,'CONNECTED','2026-01-01','2026-01-01')",
            params![id, provider, crate::token_crypto::seal(&secret.to_string())],
        )
        .unwrap();
        c.execute("INSERT INTO channel_account_bots (account_id, bot_id, owner, is_default) VALUES (?1,'bot-1','user-a',1)", params![id]).unwrap();
    }

    /// A conversation the connection already has (someone messaged first).
    async fn seed_conversation(st: &Arc<AppState>, provider: &str, acct: &str, channel: &str, conv: &str, thread: Option<&str>, workspace: Option<&str>, name: &str) -> BindingRow {
        let session = crate::thread_routes::channel_thread(&st.db, &Rt, "bot-1", provider, conv, name, "o").await.unwrap();
        let tid = thread_of_session(&st.db, &session).unwrap();
        let ev = Inbound {
            kind: InboundKind::Message, workspace: workspace.map(str::to_string), channel: channel.into(), conversation: conv.into(), thread: thread.map(str::to_string),
            remote_id: "r0".into(), message_id: "r0".into(), text: Some("hi".into()), user: Some("u".into()), reaction: None, added: None, cursor: None, own: false,
        };
        let b = ensure_binding_on(&st.db, "user-a", &tid, provider, &ev, Some(acct)).unwrap();
        st.db.connect().unwrap().execute("UPDATE channel_conversation_bindings SET channel_name = ?2 WHERE id = ?1", params![b.id, name]).unwrap();
        b
    }

    fn body(bot: &str, target: Value, text: &str) -> StartBody {
        StartBody { bot_id: bot.into(), target: serde_json::from_value(target).unwrap(), text: text.into(), attachments: None, subject: None }
    }

    fn threads(st: &Arc<AppState>) -> i64 {
        st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_threads", [], |r| r.get(0)).unwrap()
    }

    macro_rules! deps {
        ($st:expr, $http:expr, $tx:expr, $mail:expr) => {{
            let tx = $tx.clone();
            let f = move |_: &str, _: &Account| Some(tx.clone() as Arc<dyn ChannelTransport>);
            (Deps { db: &$st.db, rt: &Rt, http: $http.clone() as Arc<dyn HttpSend>, transport: Box::leak(Box::new(f)), mail: $mail }, ())
        }};
    }

    fn run_start(st: &Arc<AppState>, http: &Arc<FakeHttp>, tx: &Arc<FakeTx>, mail: &FakeMail, provider: &str, b: StartBody) -> Result<(StatusCode, Value), StartError> {
        let (deps, _) = deps!(st, http, tx, mail);
        futures::executor::block_on(start_conversation(&deps, "user-a", provider, b))
    }

    fn err_code(r: Result<(StatusCode, Value), StartError>) -> (u16, String) {
        let e = r.expect_err("a refusal");
        (e.status.as_u16(), e.code)
    }

    fn first_outbound(st: &Arc<AppState>, thread: &str) -> (String, String) {
        st.db.connect().unwrap().query_row("SELECT direction, json_extract(detail_json,'$.text') FROM channel_message_log WHERE thread_id = ?1 ORDER BY created_at LIMIT 1", params![thread], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn telegram_group_the_bot_is_in_starts_a_thread_with_a_stored_outbound() {
        let st = state("tg1").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        let http = Arc::new(FakeHttp::default());
        http.get_replies.lock().unwrap().push(("getChatMember".into(), 200, json!({ "ok": true, "result": { "status": "administrator" } })));
        let tx = FakeTx::new("telegram");
        let mail = FakeMail::new();
        let (status, v) = run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", json!({ "kind": "group", "id": "-1001234" }), "Standup in 5")).unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["state"], "sent");
        assert_eq!(v["existing"], false);
        let thread = v["threadId"].as_str().unwrap();
        let b = binding_for_thread(&st.db, "user-a", thread).expect("a binding on the new thread");
        assert_eq!((b.provider.as_str(), b.conversation.as_str(), b.channel.as_deref()), ("telegram", "telegram:-1001234", Some("-1001234")));
        assert_eq!(b.account.as_deref(), Some("acct-tg"));
        assert_eq!(tx.sent.lock().unwrap()[0].text, "Standup in 5");
        assert_eq!(first_outbound(&st, thread), ("outbound".into(), "Standup in 5".into()));
        let ev: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = ?1 AND event_type = 'channel.message.sent'", params![thread], |r| r.get(0)).unwrap();
        assert_eq!(ev, 1);
        assert_eq!(v["conversationId"], "-1001234");
        // The membership check named the bot's own id.
        assert!(http.gets.lock().unwrap()[0].contains("user_id=123"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn telegram_rules_never_leave_a_thread_behind() {
        let st = state("tg2").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        let http = Arc::new(FakeHttp::default());
        http.get_replies.lock().unwrap().push(("getChatMember".into(), 200, json!({ "ok": true, "result": { "status": "left" } })));
        let tx = FakeTx::new("telegram");
        let mail = FakeMail::new();
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", json!({ "kind": "user", "id": "777" }), "hi"))), (409, "user_never_messaged_bot".into()));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", json!({ "kind": "group", "id": "-100" }), "hi"))), (409, "bot_not_in_group".into()));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", json!({ "kind": "user", "id": "x7" }), "hi"))), (400, "invalid_target".into()));
        assert_eq!(threads(&st), 0);
        assert_eq!(tx.count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn telegram_person_who_messaged_first_continues_their_thread() {
        let st = state("tg3").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        let seeded = seed_conversation(&st, "telegram", "acct-tg", "777", "telegram:777", None, None, "Eoj").await;
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("telegram");
        let mail = FakeMail::new();
        let (_, v) = run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", json!({ "kind": "user", "id": "777" }), "Following up")).unwrap();
        assert_eq!(v["threadId"], seeded.thread_id.as_str());
        assert_eq!(v["existing"], true);
        assert_eq!(threads(&st), 1);
        assert!(http.gets.lock().unwrap().is_empty(), "a known chat needs no platform lookup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn telegram_username_resolves_through_get_chat() {
        let st = state("tg4").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        let http = Arc::new(FakeHttp::default());
        http.get_replies.lock().unwrap().push(("getChat?chat_id=%40allternit_news".into(), 200, json!({ "ok": true, "result": { "id": -1009, "type": "channel" } })));
        http.get_replies.lock().unwrap().push(("getChatMember".into(), 200, json!({ "ok": true, "result": { "status": "member" } })));
        let tx = FakeTx::new("telegram");
        let mail = FakeMail::new();
        let (_, v) = run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", json!({ "kind": "channel", "username": "@allternit_news" }), "Launch day")).unwrap();
        assert_eq!(v["conversationId"], "-1009");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn slack_channel_post_binds_the_conversation_to_its_ts() {
        let st = state("sl1").await;
        account(&st, "acct-sl", "slack", json!({ "teamId": "T1" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("slack");
        let mail = FakeMail::new();
        let (_, v) = run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "channel", "id": "C123" }), "Deploy done")).unwrap();
        assert_eq!(v["conversationId"], "C123:1700000000.000100");
        let b = binding_for_thread(&st.db, "user-a", v["threadId"].as_str().unwrap()).unwrap();
        assert_eq!(b.conversation, "slack:C123:1700000000.000100");
        assert_eq!(b.external_thread.as_deref(), Some("1700000000.000100"));
        // The first post was top-level, and a reply in the Slack thread finds the binding.
        assert_eq!(tx.sent.lock().unwrap()[0].thread, None);
        assert!(find_binding(&st.db, "slack", "slack:C123:1700000000.000100").is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn slack_dm_goes_to_the_user_id_and_refusals_have_codes() {
        let st = state("sl2").await;
        account(&st, "acct-sl", "slack", json!({ "teamId": "T1" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("slack");
        let mail = FakeMail::new();
        run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "user", "id": "U777" }), "hello")).unwrap();
        assert_eq!(tx.sent.lock().unwrap()[0].channel, "U777");
        tx.answer(Err(PostError::Rejected("cloud slack send refused: not_in_channel".into())));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "channel", "id": "C9" }), "x"))), (409, "bot_not_in_channel".into()));
        tx.answer(Err(PostError::Rejected("cloud slack send refused: channel_not_found".into())));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "channel", "id": "C8" }), "x"))), (404, "channel_not_found".into()));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "channel", "id": "general" }), "x"))), (400, "invalid_target".into()));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "channel", "id": "U1" }), "x"))), (400, "invalid_target".into()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discord_channel_ok_dm_and_webhook_connections_are_honest() {
        let st = state("dc1").await;
        account(&st, "acct-dc", "discord", json!({ "mode": "app", "guildId": "G1", "cloudToken": "t" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("discord");
        let mail = FakeMail::new();
        let (_, v) = run_start(&st, &http, &tx, &mail, "discord", body("bot-1", json!({ "kind": "channel", "id": "555" }), "Release notes")).unwrap();
        assert_eq!(v["state"], "sent");
        assert_eq!(find_binding(&st.db, "discord", "discord:555").unwrap().thread_id, v["threadId"].as_str().unwrap());
        tx.answer(Err(PostError::Rejected("cloud returned 403: channel_not_in_guild".into())));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "discord", body("bot-1", json!({ "kind": "channel", "id": "556" }), "x"))), (409, "channel_not_in_guild".into()));
        // A legacy channel-webhook connection can only continue what it has seen.
        let st2 = state("dc2").await;
        account(&st2, "acct-dc", "discord", json!({ "webhookUrl": "https://discord.com/api/webhooks/1/x" }));
        assert_eq!(err_code(run_start(&st2, &http, &tx, &mail, "discord", body("bot-1", json!({ "kind": "channel", "id": "555" }), "x"))), (409, "discord_webhook_fixed_channel".into()));
        // ...and cannot reach a person's DMs at all.
        assert_eq!(err_code(run_start(&st2, &http, &tx, &mail, "discord", body("bot-1", json!({ "kind": "user", "id": "123456789012345678" }), "x"))), (409, "discord_dm_unavailable".into()));
    }

    const DM_USER: &str = "123456789012345678";

    fn dm_target() -> Value {
        json!({ "kind": "user", "id": DM_USER })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discord_dm_opens_the_channel_first_and_binds_replies_to_it() {
        let _cloud_url = crate::test_helpers::cloud_url_env(Some("https://api.test"));
        let st = state("dm1").await;
        account(&st, "acct-dc", "discord", json!({ "mode": "app", "guildId": "G1", "cloudToken": "t" }));
        let (http, tx, mail) = (Arc::new(FakeHttp::default()), FakeTx::new("discord"), FakeMail::new());
        *http.post_reply.lock().unwrap() = Some((200, json!({ "channelId": "900100200300400500" })));
        let (status, v) = run_start(&st, &http, &tx, &mail, "discord", body("bot-1", dm_target(), "Hi, it's Finance")).unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["state"], "sent");
        // The cloud was asked only to open the DM (no text), scoped to the connection's server.
        let asked = http.posts.lock().unwrap()[0].clone();
        assert_eq!(asked.url, "https://api.test/api/v1/channels/discord/dm");
        assert_eq!(asked.body, json!({ "userId": DM_USER, "guildId": "G1" }));
        // Replies arrive on the DM channel, so that is the conversation key.
        assert_eq!(find_binding(&st.db, "discord", "discord:900100200300400500").unwrap().thread_id, v["threadId"].as_str().unwrap());
        let out = tx.sent.lock().unwrap()[0].clone();
        assert_eq!((out.channel.as_str(), out.workspace.as_deref()), ("900100200300400500", Some("@dm:123456789012345678")));
        // A second start finds the same conversation instead of making another thread.
        let (_, again) = run_start(&st, &http, &tx, &mail, "discord", body("bot-1", dm_target(), "Following up")).unwrap();
        assert_eq!((again["existing"].clone(), again["threadId"].clone()), (json!(true), v["threadId"].clone()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discord_dm_refusals_leave_nothing_behind() {
        let _cloud_url = crate::test_helpers::cloud_url_env(Some("https://api.test"));
        let st = state("dm2").await;
        account(&st, "acct-dc", "discord", json!({ "mode": "app", "guildId": "G1", "cloudToken": "t" }));
        let (http, tx, mail) = (Arc::new(FakeHttp::default()), FakeTx::new("discord"), FakeMail::new());
        for (status, error, want) in [
            (403, "user_not_in_server", (409, "discord_user_not_in_server")),
            (403, "dm_closed", (409, "discord_dm_closed")),
            (404, "discord_not_installed", (409, "not_connected")),
            (503, "discord_not_configured", (503, "discord_not_configured")),
        ] {
            *http.post_reply.lock().unwrap() = Some((status, json!({ "error": error })));
            assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "discord", body("bot-1", dm_target(), "x"))), (want.0, want.1.into()), "{error}");
        }
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "discord", body("bot-1", json!({ "kind": "user", "id": "@alice" }), "x"))), (400, "invalid_target".into()));
        assert_eq!((threads(&st), tx.count()), (0, 0));
        // The DM opened, but Discord then refuses the message itself: the platform's reason, in the codes the UI explains.
        *http.post_reply.lock().unwrap() = Some((200, json!({ "channelId": "900100200300400500" })));
        tx.answer(Err(PostError::Rejected("discord_dm_closed".into())));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "discord", body("bot-1", dm_target(), "x"))), (409, "discord_dm_closed".into()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attachments_go_where_the_platform_allows_and_are_checked_before_any_thread_exists() {
        use base64::Engine as _;
        let file = |name: &str, mime: &str, bytes: &[u8]| json!({ "filename": name, "mimeType": mime, "dataBase64": base64::engine::general_purpose::STANDARD.encode(bytes) });
        let st = state("att").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        account(&st, "acct-sl", "slack", json!({ "botToken": "xoxb-1" }));
        account(&st, "acct-dc", "discord", json!({ "mode": "app", "guildId": "G1", "cloudToken": "t" }));
        let (http, mail) = (Arc::new(FakeHttp::default()), FakeMail::new());
        http.get_replies.lock().unwrap().push(("getChatMember".into(), 200, json!({ "ok": true, "result": { "status": "administrator" } })));
        // Telegram and Slack hand the files to the transport, and the log names them.
        let tg = FakeTx::new("telegram");
        let mut b = body("bot-1", json!({ "kind": "group", "id": "-1001" }), "Here's the report");
        b.attachments = Some(vec![file("report.pdf", "application/pdf", b"%PDF"), file("chart.png", "image/png", b"png")]);
        let (_, v) = run_start(&st, &http, &tg, &mail, "telegram", b).unwrap();
        assert_eq!(*tg.files.lock().unwrap(), vec![vec!["report.pdf".to_string(), "chart.png".to_string()]]);
        let detail: String = st.db.connect().unwrap().query_row("SELECT detail_json FROM channel_message_log WHERE thread_id = ?1 AND direction = 'outbound'", params![v["threadId"].as_str().unwrap()], |r| r.get(0)).unwrap();
        assert!(detail.contains("report.pdf"), "{detail}");
        let sl = FakeTx::new("slack");
        let mut b = body("bot-1", json!({ "kind": "channel", "id": "C1" }), "Notes attached");
        b.attachments = Some(vec![file("notes.txt", "text/plain", b"hi")]);
        run_start(&st, &http, &sl, &mail, "slack", b).unwrap();
        assert_eq!(sl.files.lock().unwrap().len(), 1);
        // Email attaches through mailflare.
        email_channel(&st, true);
        let mut b = body("bot-1", json!({ "kind": "user", "email": "pat@example.com" }), "Quote attached");
        b.attachments = Some(vec![file("quote.pdf", "application/pdf", b"%PDF")]);
        run_start(&st, &http, &FakeTx::new("telegram"), &mail, "email", b).unwrap();
        assert_eq!(*mail.files.lock().unwrap(), vec![vec!["quote.pdf".to_string()]]);
        // Platforms with no bot-attachment path say so and create nothing.
        let before = threads(&st);
        let dc = FakeTx::new("discord");
        for provider in ["discord", "teams", "whatsapp"] {
            let mut b = body("bot-1", json!({ "kind": "user", "id": "+14155550123" }), "x");
            b.attachments = Some(vec![file("a.png", "image/png", b"x")]);
            assert_eq!(err_code(run_start(&st, &http, &dc, &mail, provider, b)), (400, "attachments_unsupported".into()), "{provider}");
        }
        // Size caps: 413 with a sentence, before anything is created.
        let mut b = body("bot-1", json!({ "kind": "group", "id": "-1001" }), "big");
        b.attachments = Some(vec![file("big.bin", "application/octet-stream", &vec![0u8; crate::channel_files::MAX_EACH_BYTES + 1])]);
        assert_eq!(err_code(run_start(&st, &http, &tg, &mail, "telegram", b)), (413, "attachment_too_large".into()));
        let mut b = body("bot-1", json!({ "kind": "group", "id": "-1001" }), "many");
        b.attachments = Some((0..6).map(|i| file(&format!("{i}.txt"), "text/plain", b"x")).collect());
        assert_eq!(err_code(run_start(&st, &http, &tg, &mail, "telegram", b)), (413, "attachment_too_large".into()));
        assert_eq!(threads(&st), before);
    }

    #[test]
    fn a_vendor_bot_hears_one_plain_sentence() {
        let sent = vendor_result(Ok((StatusCode::OK, json!({ "threadId": "t1", "provider": "slack", "state": "sent", "remoteId": "x" }))));
        assert_eq!(sent.unwrap(), json!({ "threadId": "t1", "provider": "slack", "state": "sent", "waitingForApproval": false }));
        let waiting = vendor_result(Ok((StatusCode::ACCEPTED, json!({ "threadId": "t2", "provider": "email", "state": "pending_approval" })))).unwrap();
        assert_eq!(waiting["waitingForApproval"], true);
        assert_eq!(vendor_result(Err(conflict("user_never_messaged_bot", "Telegram bots can only message a person who has messaged the bot first."))).unwrap_err(), "Telegram bots can only message a person who has messaged the bot first.");
        assert_eq!(vendor_result(Err(StartError::new(StatusCode::PRECONDITION_REQUIRED, "approval_required", "This message needs your approval before it is sent."))).unwrap_err(), "That needs the owner's approval in Allternit first.");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn teams_needs_a_stored_conversation_reference() {
        let st = state("tm1").await;
        account(&st, "acct-tm", "teams", json!({ "appId": "a", "appPassword": "p" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("teams");
        let mail = FakeMail::new();
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "teams", body("bot-1", json!({ "kind": "user", "id": "a:1xyz" }), "hi"))), (409, "no_conversation_reference".into()));
        assert_eq!(threads(&st), 0);
        seed_conversation(&st, "teams", "acct-tm", "a:1xyz", "teams:a:1xyz", None, Some("https://smba.trafficmanager.net/emea/"), "Dana").await;
        let (_, v) = run_start(&st, &http, &tx, &mail, "teams", body("bot-1", json!({ "kind": "user", "id": "a:1xyz" }), "Quick question")).unwrap();
        assert_eq!(v["existing"], true);
        let out = tx.sent.lock().unwrap()[0].clone();
        assert_eq!((out.channel.as_str(), out.workspace.as_deref()), ("a:1xyz", Some("https://smba.trafficmanager.net/emea/")));
        // The cloud saying it has no reference maps to the same code.
        tx.answer(Err(PostError::Rejected("cloud teams send returned 404: conversation reference not found".into())));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "teams", body("bot-1", json!({ "kind": "user", "id": "a:1xyz" }), "again"))).1, "no_conversation_reference");
    }

    fn inbound_log(st: &Arc<AppState>, binding: &BindingRow, at: chrono::DateTime<chrono::Utc>, n: &str) {
        st.db.connect().unwrap().execute(
            "INSERT INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, correlation_id, state, detail_json, created_at, updated_at) VALUES (?1,'user-a',?2,?3,'inbound','message',?4,?4,'confirmed','{}',?5,?5)",
            params![format!("log-{n}"), binding.id, binding.thread_id, format!("wamid.{n}"), at.to_rfc3339()],
        ).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn whatsapp_free_text_only_inside_the_24_hour_window() {
        let st = state("wa1").await;
        account(&st, "acct-wa", "whatsapp", json!({ "mode": "business", "phoneNumberId": "PN1" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("whatsapp");
        let mail = FakeMail::new();
        let t = json!({ "kind": "user", "phone": "+1 415 555 0123" });
        // Never wrote: outside.
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "whatsapp", body("bot-1", t.clone(), "hi"))), (409, "outside_24h_window".into()));
        let b = seed_conversation(&st, "whatsapp", "acct-wa", "PN1", "whatsapp:PN1:14155550123", Some("14155550123"), None, "Sam").await;
        inbound_log(&st, &b, chrono::Utc::now() - chrono::Duration::hours(25), "old");
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "whatsapp", body("bot-1", t.clone(), "hi"))), (409, "outside_24h_window".into()));
        assert_eq!(tx.count(), 0);
        inbound_log(&st, &b, chrono::Utc::now() - chrono::Duration::hours(2), "new");
        let (_, v) = run_start(&st, &http, &tx, &mail, "whatsapp", body("bot-1", t, "Your order shipped")).unwrap();
        assert_eq!(v["threadId"], b.thread_id.as_str());
        let out = tx.sent.lock().unwrap()[0].clone();
        assert_eq!((out.channel.as_str(), out.thread.as_deref()), ("PN1", Some("14155550123")));
        // The cloud's own refusal (409) surfaces with the same code.
        tx.answer(Err(PostError::Rejected("outside_24h_window: template needed".into())));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "whatsapp", body("bot-1", json!({ "kind": "user", "id": "14155550123" }), "x"))).1, "outside_24h_window");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn whatsapp_personal_starts_with_a_phone_number() {
        std::env::set_var("ALLTERNIT_WA_PERSONAL", "1");
        let st = state("wp1").await;
        account(&st, "acct-wp", "whatsapp-personal", json!({ "sidecarUrl": "http://127.0.0.1:1", "sidecarToken": "t" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("whatsapp-personal");
        let mail = FakeMail::new();
        let (_, v) = run_start(&st, &http, &tx, &mail, "whatsapp-personal", body("bot-1", json!({ "kind": "user", "phone": "+44 7700 900123" }), "hello")).unwrap();
        assert_eq!(tx.sent.lock().unwrap()[0].channel, "447700900123@s.whatsapp.net");
        assert_eq!(find_binding(&st.db, "whatsapp-personal", "whatsapp-personal:447700900123@s.whatsapp.net").unwrap().thread_id, v["threadId"].as_str().unwrap());
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "whatsapp-personal", body("bot-1", json!({ "kind": "user", "phone": "12" }), "x"))).1, "invalid_target");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sms_goes_through_the_outbound_phone_path_and_shares_the_phone_thread() {
        let st = state("sms1").await;
        crate::channel_phone::upsert_number(&st.db, "num-1", "user-a", "bot-1", "+14155550100", Some("acct-sms")).unwrap();
        account(&st, "acct-sms", "sms", json!({ "numberId": "num-1", "token": "tok", "cloudUrl": "https://cloud.test" }));
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("sms");
        let mail = FakeMail::new();
        let (_, v) = run_start(&st, &http, &tx, &mail, "phone", body("bot-1", json!({ "kind": "user", "phone": "+1 (415) 555-0199" }), "Running 10 min late")).unwrap();
        assert_eq!((v["state"].as_str(), v["existing"].clone(), v["conversationId"].as_str()), (Some("sent"), json!(false), Some("+14155550100:+14155550199")));
        let b = binding_for_thread(&st.db, "user-a", v["threadId"].as_str().unwrap()).unwrap();
        assert_eq!(b.conversation, "phone:+14155550100:+14155550199");
        // The cloud send route got the text; the transport fake wasn't involved (existing outbound path).
        let posts = http.posts.lock().unwrap();
        assert_eq!(posts[0].url, "https://cloud.test/api/v1/channels/sms/send");
        assert_eq!(posts[0].body, json!({ "numberId": "num-1", "to": "+14155550199", "text": "Running 10 min late" }));
        drop(posts);
        assert_eq!(tx.count(), 0);
        assert_eq!(first_outbound(&st, &b.thread_id), ("outbound".into(), "Running 10 min late".into()));
        // A call from the same person later resolves to this thread.
        let (thread, _) = crate::channel_phone::resolve_thread_async(&st.db, &Rt, "num-1", "+14155550199").await.unwrap();
        assert_eq!(thread, b.thread_id);
        // Texting them again continues it.
        let (_, again) = run_start(&st, &http, &tx, &mail, "sms", body("bot-1", json!({ "kind": "user", "id": "+14155550199" }), "Here now")).unwrap();
        assert_eq!((again["threadId"].as_str(), again["existing"].clone()), (Some(b.thread_id.as_str()), json!(true)));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "sms", body("bot-1", json!({ "kind": "user", "phone": "5550199" }), "x"))).1, "invalid_target");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sms_to_someone_who_never_contacted_the_number_is_a_no_consent_refusal() {
        let st = state("sms2").await;
        crate::channel_phone::upsert_number(&st.db, "num-1", "user-a", "bot-1", "+14155550100", Some("acct-sms")).unwrap();
        account(&st, "acct-sms", "sms", json!({ "numberId": "num-1", "token": "tok", "cloudUrl": "https://cloud.test" }));
        let http = Arc::new(FakeHttp::default());
        *http.post_reply.lock().unwrap() = Some((403, json!({ "error": "no_consent" })));
        let (tx, mail) = (FakeTx::new("sms"), FakeMail::new());
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "sms", body("bot-1", json!({ "kind": "user", "phone": "+14155550177" }), "hi"))), (403, "no_consent".into()));
        // A bot with no number of its own.
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "sms", body("bot-9", json!({ "kind": "user", "phone": "+14155550177" }), "hi"))).1, "bot_not_found");
    }

    fn email_channel(st: &Arc<AppState>, send_enabled: bool) {
        st.db.connect().unwrap().execute(
            "INSERT INTO agent_identity_channels (id, agent_id, user_id, email_provider, email_address, email_send_enabled, email_receive_enabled, email_mailbox_id) VALUES ('aic-1','bot-1','user-a','mailflare','scout@bots.allternit.com',?1,1,'mb-1')",
            params![send_enabled as i64],
        ).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn email_composes_through_the_approval_gate_and_threads_like_inbound() {
        let st = state("em1").await;
        let http = Arc::new(FakeHttp::default());
        let tx = FakeTx::new("email");
        let mail = FakeMail::new();
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "email", body("bot-1", json!({ "kind": "user", "email": "pat@example.com" }), "hi"))), (409, "not_connected".into()));
        email_channel(&st, true);
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "email", body("bot-1", json!({ "kind": "user", "email": "not-an-email" }), "hi"))).1, "invalid_target");
        let (status, v) = run_start(&st, &http, &tx, &mail, "email", body("bot-1", json!({ "kind": "user", "email": "Pat@Example.com" }), "Pricing for H100?\n\nWe'd like annual terms.")).unwrap();
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(v["state"], "pending_approval");
        assert_eq!(v["approvalThread"], "mail:email-out-out-1");
        assert_eq!(mail.sent.lock().unwrap()[0].1, "Pricing for H100?");
        // Same key as an inbound reply (sender + subject) so the answer continues this thread.
        let thread = v["threadId"].as_str().unwrap();
        let origin: String = st.db.connect().unwrap().query_row("SELECT json_extract(origin,'$.channelKey') FROM bot_threads WHERE id = ?1", params![thread], |r| r.get(0)).unwrap();
        assert_eq!(origin, "email:pat@example.com:pricing for h100?");
        let ev: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = ?1 AND event_type = 'channel.message.pending'", params![thread], |r| r.get(0)).unwrap();
        assert_eq!(ev, 1);
        // A refused send (mailflare down) leaves no new thread.
        *mail.reply.lock().unwrap() = Err((StatusCode::BAD_GATEWAY, json!({ "error": "mailflare_send_failed", "message": "down" })));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "email", body("bot-1", json!({ "kind": "user", "email": "lee@example.com" }), "Other thing"))), (502, "mailflare_send_failed".into()));
        assert_eq!(threads(&st), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn email_send_switched_off_is_refused() {
        let st = state("em2").await;
        email_channel(&st, false);
        let (http, tx, mail) = (Arc::new(FakeHttp::default()), FakeTx::new("email"), FakeMail::new());
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "email", body("bot-1", json!({ "kind": "user", "email": "pat@example.com" }), "hi"))), (403, "email_send_disabled".into()));
        assert!(mail.sent.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn request_validation_and_ownership() {
        let st = state("val").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        let (http, tx, mail) = (Arc::new(FakeHttp::default()), FakeTx::new("telegram"), FakeMail::new());
        let t = json!({ "kind": "user", "id": "7" });
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "telegram", body("bot-1", t.clone(), "  "))), (400, "text_required".into()));
        let mut with_files = body("bot-1", t.clone(), "hi");
        with_files.attachments = Some(vec![json!({ "name": "a.pdf" })]);
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "telegram", with_files)), (400, "invalid_attachment".into()), "a file with no data is refused before any thread exists");
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "carrier-pigeon", body("bot-1", t.clone(), "hi"))), (404, "unknown_provider".into()));
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "telegram", body("bot-9", t.clone(), "hi"))), (404, "bot_not_found".into()));
        // A bot with no connection of its own.
        assert_eq!(err_code(run_start(&st, &http, &tx, &mail, "slack", body("bot-1", json!({ "kind": "channel", "id": "C1" }), "hi"))), (409, "not_connected".into()));
        assert_eq!(threads(&st), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_channel_policy_that_asks_holds_the_first_message_for_approval() {
        let st = state("pol").await;
        account(&st, "acct-dc", "discord", json!({ "mode": "app", "guildId": "G1", "cloudToken": "t" }));
        let b = body("bot-1", json!({ "kind": "channel", "id": "555" }), "Hello server");
        let (http, tx, mail) = (Arc::new(FakeHttp::default()), FakeTx::new("discord"), FakeMail::new());
        let (deps, _) = deps!(st, http, tx, &mail);
        // The bot's discord send policy is "ask": starting holds the first message.
        st.db.connect().unwrap().execute("UPDATE agents SET config = ?1 WHERE id = 'bot-1'", params![json!({ "channelTools": { "discord": { "ask": ["channel.send"] } } }).to_string()]).unwrap();
        let e = start_conversation(&deps, "user-a", "discord", b).await.expect_err("held for approval");
        assert_eq!((e.status.as_u16(), e.code.as_str()), (428, "approval_required"));
        assert!(e.extra["threadId"].is_string() && e.extra["approvalId"].is_string());
        assert_eq!(tx.count(), 0, "nothing went out");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn targets_list_known_conversations_with_the_start_rules() {
        let st = state("tgt").await;
        account(&st, "acct-tg", "telegram", json!({ "botToken": "123:abc" }));
        seed_conversation(&st, "telegram", "acct-tg", "777", "telegram:777", None, None, "Eoj").await;
        seed_conversation(&st, "telegram", "acct-tg", "-1001", "telegram:-1001", None, None, "Ops chat").await;
        let http = FakeHttp::default();
        let v = list_targets(&st.db, &http, "user-a", "telegram", "bot-1", None).await.unwrap();
        let t: Vec<(String, String, String)> = v["targets"].as_array().unwrap().iter().map(|t| (t["kind"].as_str().unwrap().into(), t["id"].as_str().unwrap().into(), t["name"].as_str().unwrap().into())).collect();
        assert!(t.contains(&("user".into(), "777".into(), "Eoj".into())));
        assert!(t.contains(&("group".into(), "-1001".into(), "Ops chat".into())));
        assert_eq!(v["canStartWith"], json!(["user", "group", "channel", "username"]));
        assert_eq!(v["connected"], true);
        assert!(v["notes"].as_str().unwrap().contains("messaged it first"));
        // Another owner's bot, an unconnected provider and a bad provider.
        assert_eq!(list_targets(&st.db, &http, "user-a", "telegram", "bot-9", None).await.unwrap_err().status.as_u16(), 404);
        let none = list_targets(&st.db, &http, "user-a", "teams", "bot-1", None).await.unwrap();
        assert_eq!((none["connected"].clone(), none["targets"].clone()), (json!(false), json!([])));
        assert_eq!(list_targets(&st.db, &http, "user-a", "fax", "bot-1", None).await.unwrap_err().code, "unknown_provider");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn slack_targets_add_the_channels_the_bot_joined() {
        let st = state("tgs").await;
        account(&st, "acct-sl", "slack", json!({ "teamId": "T1" }));
        seed_conversation(&st, "slack", "acct-sl", "C1", "slack:C1:1.1", Some("1.1"), None, "#general").await;
        let http = FakeHttp::default();
        *http.form.lock().unwrap() = Some(json!({ "ok": true, "channels": [{ "id": "C1", "name": "general", "is_member": true }, { "id": "C2", "name": "eng", "is_member": true }, { "id": "C3", "name": "random", "is_member": false }] }));
        let v = list_targets(&st.db, &http, "user-a", "slack", "bot-1", Some("xoxb-1")).await.unwrap();
        let ids: Vec<&str> = v["targets"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["C1", "C2"], "known first, joined added once, non-member left out");
        let v = list_targets(&st.db, &http, "user-a", "slack", "bot-1", None).await.unwrap();
        assert_eq!(v["targets"].as_array().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn whatsapp_targets_only_include_people_inside_the_window() {
        let st = state("tgw").await;
        account(&st, "acct-wa", "whatsapp", json!({ "mode": "business", "phoneNumberId": "PN1" }));
        let fresh = seed_conversation(&st, "whatsapp", "acct-wa", "PN1", "whatsapp:PN1:111111111", Some("111111111"), None, "Fresh").await;
        let stale = seed_conversation(&st, "whatsapp", "acct-wa", "PN1", "whatsapp:PN1:222222222", Some("222222222"), None, "Stale").await;
        inbound_log(&st, &fresh, chrono::Utc::now() - chrono::Duration::hours(1), "f");
        inbound_log(&st, &stale, chrono::Utc::now() - chrono::Duration::hours(30), "s");
        let v = list_targets(&st.db, &FakeHttp::default(), "user-a", "whatsapp", "bot-1", None).await.unwrap();
        let ids: Vec<&str> = v["targets"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["111111111"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn email_targets_and_mail_threads_by_folder() {
        let st = state("mail").await;
        email_channel(&st, true);
        let c = st.db.connect().unwrap();
        for (i, from, subj, status, at) in [
            ("in-1", "pat@example.com", "Pricing", Some("sent"), "2026-10-01 10:00:00"),
            ("in-2", "pat@example.com", "Re: Pricing", None::<&str>, "2026-10-02 09:00:00"),
            ("in-3", "lee@example.com", "Hello", None, "2026-10-02 11:00:00"),
        ] {
            c.execute("INSERT INTO agent_email_inbound (id, agent_id, from_address, subject, snippet, created_at, reply_status) VALUES (?1,'bot-1',?2,?3,'snip',?4,?5)", params![i, from, subj, at, status]).unwrap();
        }
        c.execute("INSERT INTO agent_email_inbound (id, agent_id, from_address, subject, snippet, created_at, guard_reason, reply_status) VALUES ('in-4','bot-1','news@list.com','Deal','x','2026-10-02 12:00:00','list','skipped')", []).unwrap();
        for (i, to, status, at) in [("o-1", "pat@example.com", "pending_approval", "2026-10-02 12:00:00"), ("o-2", "lee@example.com", "sent", "2026-10-01 08:00:00")] {
            c.execute(
                "INSERT INTO agent_email_outbound (id, agent_id, user_id, thread_id, idempotency_key, to_address, subject, snippet, status, created_at) VALUES (?1,'bot-1','user-a',?2,?2,?3,'Quote','body',?4,?5)",
                params![i, format!("mail:email-out-{i}"), to, status, at],
            ).unwrap();
        }
        let inbox = mail_threads(&st.db, "user-a", "bot-1", "inbox").unwrap();
        let t = inbox["threads"].as_array().unwrap();
        assert_eq!(t.len(), 2, "one thread per sender+subject, guarded mail left out: {t:?}");
        assert_eq!((t[0]["from"].as_str(), t[0]["unread"].clone()), (Some("lee@example.com"), json!(true)));
        assert_eq!((t[1]["subject"].as_str(), t[1]["unread"].clone(), t[1]["at"].as_str()), (Some("Re: Pricing"), json!(true), Some("2026-10-02T09:00:00Z")));
        let ok = mail_threads(&st.db, "user-a", "bot-1", "needs_ok").unwrap();
        assert_eq!(ok["threads"][0]["id"], "mail:email-out-o-1");
        assert_eq!((ok["threads"][0]["needsApproval"].clone(), ok["threads"][0]["from"].clone()), (json!(true), json!("scout@bots.allternit.com")));
        let sent = mail_threads(&st.db, "user-a", "bot-1", "sent").unwrap();
        assert_eq!(sent["threads"].as_array().unwrap().len(), 1);
        assert!(sent["threads"][0].get("needsApproval").is_none());
        assert_eq!(mail_threads(&st.db, "user-a", "bot-1", "trash").unwrap_err().code, "invalid_folder");
        assert_eq!(mail_threads(&st.db, "user-a", "bot-9", "inbox").unwrap_err().status.as_u16(), 404);
        let v = list_targets(&st.db, &FakeHttp::default(), "user-a", "email", "bot-1", None).await.unwrap();
        let ids: Vec<&str> = v["targets"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"pat@example.com") && ids.contains(&"lee@example.com") && !ids.contains(&"news@list.com"));
        // An answered inbound thread is read, and points at the bot's thread when there is one.
        let session = crate::thread_routes::channel_thread(&st.db, &Rt, "bot-1", "email", "email:lee@example.com:hello", "Hello", "o").await.unwrap();
        let tid = thread_of_session(&st.db, &session).unwrap();
        let inbox = mail_threads(&st.db, "user-a", "bot-1", "inbox").unwrap();
        assert_eq!(inbox["threads"][0]["id"], tid.as_str());
    }

    #[test]
    fn the_router_registers_without_overlapping_itself() {
        // axum panics at build time on overlapping routes.
        let _ = channel_start_router();
        assert_eq!(canonical("phone"), Some("sms"));
        assert_eq!(canonical("whatsapp-personal"), Some("whatsapp-personal"));
        assert_eq!(canonical("email"), Some("email"));
        assert_eq!(canonical("nope"), None);
    }
}
