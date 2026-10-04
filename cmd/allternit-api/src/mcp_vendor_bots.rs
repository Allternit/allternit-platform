//! The vendor-bot MCP connector: `mcp.allternit.com/mcp/bots/<vendorBotId>`.
//!
//! A vendor bot (a Claude, ChatGPT or Grok agent bound to an Allternit bot
//! through the Agent Gateway) does **not** get a phone number of its own. It
//! shares its *directing bot's* phone and mailbox and acts through tools,
//! never by operating Allternit's UI. Same OAuth plumbing as the agents server
//! in [`crate::mcp_agents`] (Clerk, RFC 9728 metadata per path), with its own
//! scope: `bots:act`, separate from `agents:read`.
//!
//! * Access: the caller's Clerk user must own the vendor bot's execution
//!   binding (anyone else gets 404, no oracle). An OAuth token must carry
//!   `aud` = this bot's connector URL and `bots:act`; a token minted for the
//!   agents server, or for another bot, is refused. An OAuth client the owner
//!   revoked is refused on its next call.
//! * Tools go through the same code paths as the bot itself, so the gates stay
//!   where they are: the cloud's consent, STOP/opt-out, carrier registration
//!   and daily cap on texts (`/api/v1/channels/sms/send`) and calls
//!   (`/api/v1/phone/calls/outbound`); mailflare's human approval on mail; the
//!   directing bot's own approval flow on `ask_bot`. A refusal comes back as a
//!   tool error in one plain sentence.
//! * "Their view": `list_threads` / `read_thread` see only threads the vendor
//!   bot is attached to, or the owner shared with it.
//! * Attribution: every outbound message says it came from the vendor bot via
//!   the directing bot, and every call writes a `vendor_bot_audit` row (who,
//!   tool, a hash of the arguments, result).
//! * Keys page: `GET /api/v1/vendor-bots/:id/connector`, `PUT` to set the
//!   directing bot, `DELETE .../connector/clients/:clientId` to revoke.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Extension, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::auth::AuthUser;
use crate::channel_phone::PhoneNumber;
use crate::phone_outbound;
use crate::channel_transports::{HttpSend, ReqwestSend};
use crate::db::DbHandle;
use crate::mcp_agents::{challenge_value_for, peek_claims, public_mcp_url, resource_metadata_url, McpTokenError};
use crate::thread_routes::ThreadRuntime;
use crate::AppState;

pub const BOT_SCOPE: &str = "bots:act";
const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2025-03-26"];
const MAX_TEXT_CHARS: usize = 4000;

pub const SERVER_INSTRUCTIONS: &str = "You act through your directing bot's phone and mailbox. send_text and start_call \
only reach people who contacted that number first or were added as contacts; STOP always wins. send_email is queued \
for the owner's approval. If a tool refuses, tell the user why in the refusal's words. When told to run an Allternit ticket, call \
get_ticket, do the work, then post_result.";

// ─── URLs and OAuth metadata ───────────────────────────────────────────────────

/// The OAuth resource (and expected `aud`) for one vendor bot's connector.
pub fn bot_resource_url(vendor_bot_id: &str) -> String {
    format!("{}/bots/{vendor_bot_id}", public_mcp_url().trim_end_matches('/'))
}

pub fn bot_protected_resource_metadata(vendor_bot_id: &str) -> Value {
    json!({
        "resource": bot_resource_url(vendor_bot_id),
        "authorization_servers": [crate::mcp_agents::oauth_issuer()],
        "scopes_supported": [BOT_SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Allternit vendor bot"
    })
}

/// The bot id in a `.well-known` path suffix like `mcp/bots/<id>`.
pub fn bot_id_from_resource_path(rest: &str) -> Option<&str> {
    let rest = rest.trim_matches('/');
    let id = rest.rsplit_once("bots/").map(|(_, id)| id)?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

// ─── The session a connector call runs as ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Session {
    pub owner: String,
    pub vendor_bot_id: String,
    pub vendor_name: String,
    pub directing_bot_id: Option<String>,
    pub directing_name: Option<String>,
    /// The OAuth client behind the call; `None` for the owner's own session.
    pub client: Option<String>,
}

impl Session {
    fn directing(&self) -> Result<(&str, &str), String> {
        match (&self.directing_bot_id, &self.directing_name) {
            (Some(id), Some(name)) => Ok((id, name)),
            _ => Err("This vendor bot has no directing bot yet. Choose which of your bots directs it, then try again.".into()),
        }
    }

    /// Who the recipient is told this came from.
    fn attribution(&self) -> String {
        match &self.directing_name {
            Some(d) => format!("{} via {d}", self.vendor_name),
            None => self.vendor_name.clone(),
        }
    }
}

/// The vendor bot `vendor_bot_id` when `owner` owns its execution binding, with
/// its directing bot. With none chosen, the owner's one phone-bearing bot.
pub fn load_session(db: &DbHandle, owner: &str, vendor_bot_id: &str) -> Option<Session> {
    let conn = db.connect().ok()?;
    let vendor_name: String = conn
        .query_row(
            "SELECT a.name FROM agents a JOIN bot_execution_bindings b ON b.bot_id = a.id
             WHERE a.id = ?1 AND a.user_id = ?2 AND b.owner = ?2 AND b.type = 'vendor'",
            params![vendor_bot_id, owner],
            |r| r.get(0),
        )
        .optional()
        .ok()??;
    let chosen: Option<String> = conn
        .query_row("SELECT directing_bot_id FROM vendor_bot_connectors WHERE vendor_bot_id = ?1 AND owner = ?2", params![vendor_bot_id, owner], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
        .flatten();
    let bot_name = |id: &str| -> Option<String> {
        conn.query_row("SELECT name FROM agents WHERE id = ?1 AND user_id = ?2 AND id <> ?3", params![id, owner, vendor_bot_id], |r| r.get(0)).optional().ok().flatten()
    };
    let directing = chosen.and_then(|id| bot_name(&id).map(|n| (id, n))).or_else(|| {
        let mut q = conn
            .prepare("SELECT DISTINCT p.bot_id FROM channel_phone_numbers p WHERE p.owner = ?1 AND p.bot_id <> ?2 LIMIT 2")
            .ok()?;
        let ids: Vec<String> = q.query_map(params![owner, vendor_bot_id], |r| r.get(0)).ok()?.filter_map(Result::ok).collect();
        match ids.as_slice() {
            [only] => bot_name(only).map(|n| (only.clone(), n)),
            _ => None,
        }
    });
    Some(Session {
        owner: owner.to_string(),
        vendor_bot_id: vendor_bot_id.to_string(),
        vendor_name,
        directing_bot_id: directing.as_ref().map(|d| d.0.clone()),
        directing_name: directing.map(|d| d.1),
        client: None,
    })
}

// ─── Tool descriptors ──────────────────────────────────────────────────────────

fn annotations(read_only: bool, open_world: bool) -> Value {
    json!({ "readOnlyHint": read_only, "destructiveHint": false, "idempotentHint": read_only, "openWorldHint": open_world })
}

/// The ten tools; the ones with a card carry its `ui://` resource in `_meta`.
pub fn tool_descriptors() -> Vec<Value> {
    let mut tools = base_tool_descriptors();
    for t in &mut tools {
        if let Some(uri) = t["name"].as_str().and_then(crate::mcp_vendor_cards::card_uri) {
            t["_meta"] = crate::mcp_vendor_cards::tool_meta(uri);
        }
    }
    tools
}

/// Plain-text instructions an agent can read to learn the tools (also served by the cloud edge
/// at `/bots/:id/instructions` and printed by `allternit-bot help`).
pub const BOT_INSTRUCTIONS: &str = include_str!("../assets/vendor-bot-instructions.txt");

fn base_tool_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "list_threads", "title": "List threads",
            "description": "List the conversations you can see: threads you are attached to, and ones your directing bot shared with you. Optionally filter by channel (sms, email, slack...) or a search word.",
            "inputSchema": { "type": "object", "properties": {
                "channel": { "type": "string", "description": "Only threads on this channel, e.g. sms." },
                "query": { "type": "string", "description": "Only threads whose title, objective or summary has this text." }
            }, "additionalProperties": false },
            "annotations": annotations(true, false)
        }),
        json!({
            "name": "read_thread", "title": "Read a thread",
            "description": "Read the latest messages of a thread you can see (see list_threads).",
            "inputSchema": { "type": "object", "properties": {
                "threadId": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 30 }
            }, "required": ["threadId"], "additionalProperties": false },
            "annotations": annotations(true, false)
        }),
        json!({
            "name": "send_text", "title": "Send a text message",
            "description": "Send an SMS from your directing bot's phone number. Works only for people who texted or called that number first, or whom the owner added as contacts. STOP always wins. If it refuses, tell the user why.",
            "inputSchema": { "type": "object", "properties": {
                "to": { "type": "string", "description": "E.164, like +14155550123" },
                "text": { "type": "string" }
            }, "required": ["to", "text"], "additionalProperties": false },
            "annotations": annotations(false, true)
        }),
        json!({
            "name": "start_call", "title": "Start a phone call",
            "description": "Have your directing bot's phone call a number. Works only for people who texted or called that number first, or whom the owner added as contacts.",
            "inputSchema": { "type": "object", "properties": {
                "to": { "type": "string", "description": "E.164, like +14155550123" },
                "purpose": { "type": "string", "description": "What the call is for; it is the brief for the call." }
            }, "required": ["to", "purpose"], "additionalProperties": false },
            "annotations": annotations(false, true)
        }),
        json!({
            "name": "send_email", "title": "Send an email",
            "description": "Draft an email from your directing bot's address. It is never sent directly: it waits for the owner's OK.",
            "inputSchema": { "type": "object", "properties": {
                "to": { "type": "string" }, "subject": { "type": "string" }, "body": { "type": "string" }
            }, "required": ["to", "subject", "body"], "additionalProperties": false },
            "annotations": annotations(false, true)
        }),
        json!({
            "name": "post_message", "title": "Post in a channel",
            "description": "Start a conversation on a messaging channel (telegram, slack, discord, teams, whatsapp) through your directing bot.",
            "inputSchema": { "type": "object", "properties": {
                "provider": { "type": "string" },
                "target": { "type": "object", "description": "Who or where: {kind, id | username | email | phone}." },
                "text": { "type": "string" }
            }, "required": ["provider", "target", "text"], "additionalProperties": false },
            "annotations": annotations(false, true)
        }),
        json!({
            "name": "ask_bot", "title": "Ask the directing bot",
            "description": "Hand off to your directing bot (Gizzi) with a message, and get its reply.",
            "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"], "additionalProperties": false },
            "annotations": annotations(false, false)
        }),
        json!({
            "name": "get_ticket", "title": "Get a ticket",
            "description": "Read an Allternit task ticket (like T-3): the instructions, the tools you may use, and the deadline. When your chat says \"Run Allternit ticket T-n\", call this first.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string", "description": "The ticket id, like T-3." } }, "required": ["id"], "additionalProperties": false },
            "annotations": annotations(true, false)
        }),
        json!({
            "name": "post_result", "title": "Post a ticket's result",
            "description": "Finish an Allternit ticket: a short summary, optional structured data and optional https attachments. It lands in the thread as a result card. Call it once, when the work is done.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "string" }, "summary": { "type": "string" },
                "data": { "type": "object", "description": "Structured result, if any." },
                "attachments": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "url": { "type": "string", "description": "https only" } } } }
            }, "required": ["id", "summary"], "additionalProperties": false },
            "annotations": annotations(false, false)
        }),
        json!({
            "name": "list_open_tickets", "title": "List open tickets",
            "description": "List the Allternit tickets waiting on you.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": annotations(true, false)
        }),
    ]
}

pub fn is_tool(name: &str) -> bool {
    tool_descriptors().iter().any(|t| t["name"] == name)
}

// ─── Actions: the paths a tool goes through ────────────────────────────────────

/// What the five acting tools do. [`LiveActions`] is production; tests fake it
/// where they check dispatch, attribution and audit, and use `LiveActions`
/// itself (over a fake HTTP layer) where they check the gates.
#[async_trait]
pub trait Actions: Send + Sync {
    async fn send_text(&self, s: &Session, to: &str, text: &str) -> Result<Value, String>;
    async fn start_call(&self, s: &Session, to: &str, purpose: &str) -> Result<Value, String>;
    async fn send_email(&self, s: &Session, to: &str, subject: &str, body: &str) -> Result<Value, String>;
    async fn post_message(&self, s: &Session, provider: &str, target: &Value, text: &str) -> Result<Value, String>;
    async fn ask_bot(&self, s: &Session, text: &str) -> Result<Value, String>;
}

/// Starts a conversation on a messaging channel. The runtime's channel-start
/// path (`/api/v1/channels/:provider/start`) registers itself here when it is
/// built in; until then `post_message` says so.
#[async_trait]
pub trait ChannelStarter: Send + Sync {
    async fn start(&self, owner: &str, bot_id: &str, provider: &str, target: &Value, text: &str) -> Result<Value, String>;
}

static CHANNEL_STARTER: std::sync::OnceLock<Arc<dyn ChannelStarter>> = std::sync::OnceLock::new();

/// Wire the channel-start path to `post_message`. Called once at boot.
pub fn register_channel_starter(starter: Arc<dyn ChannelStarter>) {
    let _ = CHANNEL_STARTER.set(starter);
}

pub struct LiveActions<R: ThreadRuntime + 'static> {
    pub state: Arc<AppState>,
    pub rt: R,
    pub http: Arc<dyn HttpSend>,
    pub starter: Option<Arc<dyn ChannelStarter>>,
}

impl LiveActions<crate::coordinator_routes::GizziCoordinator> {
    pub fn production(state: &Arc<AppState>) -> Self {
        Self {
            state: state.clone(),
            rt: crate::coordinator_routes::GizziCoordinator { state: state.clone() },
            http: Arc::new(ReqwestSend),
            starter: CHANNEL_STARTER.get().cloned(),
        }
    }
}

/// `text` and, on its own line, who it came from.
fn attributed(s: &Session, text: &str) -> String {
    format!("{}\n\n(Sent by {}.)", text.trim(), s.attribution())
}

struct PhoneLine {
    number_id: String,
    e164: String,
    bot_id: String,
}

impl PhoneLine {
    fn number(&self, s: &Session) -> PhoneNumber {
        PhoneNumber { number_id: self.number_id.clone(), owner: s.owner.clone(), bot_id: self.bot_id.clone(), e164: self.e164.clone() }
    }
}

fn directing_line(db: &DbHandle, s: &Session) -> Result<PhoneLine, String> {
    let (bot_id, name) = s.directing()?;
    db.connect()
        .map_err(|e| e.to_string())?
        .query_row(
            "SELECT number_id, e164 FROM channel_phone_numbers WHERE owner = ?1 AND bot_id = ?2 ORDER BY created_at LIMIT 1",
            params![s.owner, bot_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .map(|(number_id, e164)| PhoneLine { number_id, e164, bot_id: bot_id.to_string() })
        .ok_or_else(|| format!("{name} doesn't have a phone number connected on this computer."))
}

/// A phone_outbound refusal as one plain sentence for a vendor bot, who isn't the owner.
fn refusal_for(e: phone_outbound::OutError, to: &str) -> String {
    match e {
        phone_outbound::OutError::NoConsent(_) => format!("{to} hasn't texted or called this number and isn't one of the owner's contacts, so I can't reach out. The owner can add them as a contact."),
        other => other.sentence(),
    }
}

#[async_trait]
impl<R: ThreadRuntime + 'static> Actions for LiveActions<R> {
    async fn send_text(&self, s: &Session, to: &str, text: &str) -> Result<Value, String> {
        let line = directing_line(&self.state.db, s)?;
        // One path with the bot's own texts: same gates, same thread writes; only the
        // "(Sent by …)" line is the vendor's.
        let (thread_id, message_id) = phone_outbound::text_attributed(&self.state.db, &self.rt, self.http.clone(), &line.number(s), to, text, Some(&s.attribution()))
            .await
            .map_err(|e| refusal_for(e, to))?;
        Ok(json!({ "sent": true, "threadId": thread_id, "messageId": message_id, "from": line.e164 }))
    }

    async fn start_call(&self, s: &Session, to: &str, purpose: &str) -> Result<Value, String> {
        if purpose.trim().is_empty() {
            return Err("Say what the call is for.".into());
        }
        let line = directing_line(&self.state.db, s)?;
        let brief = format!("Requested by {}: {}", s.attribution(), purpose.trim());
        let (thread_id, room) = phone_outbound::call(&self.state.db, &self.rt, self.http.clone(), &line.number(s), to, &brief).await.map_err(|e| refusal_for(e, to))?;
        Ok(json!({ "calling": true, "threadId": thread_id, "room": room }))
    }

    async fn send_email(&self, s: &Session, to: &str, subject: &str, body: &str) -> Result<Value, String> {
        let (bot_id, name) = s.directing()?;
        let req = crate::agent_email_routes::SendAgentEmailRequest {
            agent_id: bot_id.to_string(),
            to: to.trim().to_string(),
            subject: subject.trim().to_string(),
            text: Some(attributed(s, body)),
            html: None,
        };
        // The approval-gated path: the draft waits for the owner's OK, never skipped.
        match crate::agent_email_routes::send_email_for_user(&self.state, &s.owner, req).await {
            Ok(v) if v["status"] == "pending_approval" => Ok(json!({ "queued": true, "status": "pending_approval", "note": "Waiting for the owner's OK before it is sent." })),
            Ok(v) => Ok(json!({ "queued": true, "status": v["status"] })),
            Err((_, Json(e))) => Err(match e["error"].as_str().unwrap_or("") {
                "email_not_provisioned" => format!("{name} doesn't have an email address yet."),
                "email_send_disabled" => format!("Outbound email is switched off for {name}."),
                "mailflare_not_configured" => "Email isn't switched on for this computer yet.".into(),
                "invalid_request" => e["message"].as_str().unwrap_or("The email needs a subject and a body.").to_string(),
                _ => "The email couldn't be drafted. Try again in a moment.".into(),
            }),
        }
    }

    async fn post_message(&self, s: &Session, provider: &str, target: &Value, text: &str) -> Result<Value, String> {
        let (bot_id, _) = s.directing()?;
        let Some(starter) = self.starter.as_ref() else {
            return Err("Starting a conversation on a channel isn't available yet.".into());
        };
        starter.start(&s.owner, bot_id, provider, target, &attributed(s, text)).await
    }

    async fn ask_bot(&self, s: &Session, text: &str) -> Result<Value, String> {
        use crate::aai_facade as f;
        let (bot_id, _) = s.directing()?;
        let body = format!("[{} asks, through the Allternit connector]\n{}", s.vendor_name, text.trim());
        f::send(
            &self.state,
            &s.owner,
            f::SendIn { bot_id, thread_id: None, text: &body, correlation_id: None, consequential: false, allternit_approval_id: None, via: "vendor-mcp" },
        )
        .await
        .map_err(|e| match e.to_json()["code"].as_str() {
            Some("APPROVAL_REQUIRED") => "That needs the owner's approval in Allternit first.".to_string(),
            _ => "Your directing bot couldn't take that right now. Try again in a moment.".to_string(),
        })
    }
}

// ─── Reading: "their view" ─────────────────────────────────────────────────────

/// The threads this vendor bot may see: attached to it, or shared with it.
const VISIBLE: &str = "(t.bot_id = ?1 OR t.id IN (SELECT thread_id FROM vendor_bot_thread_shares WHERE vendor_bot_id = ?1 AND owner = ?2))";

pub fn list_threads(db: &DbHandle, s: &Session, args: &Value) -> Result<Value, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let channel = str_arg(args, "channel").map(str::to_lowercase);
    let query = str_arg(args, "query").map(|q| format!("%{}%", q.replace('%', "")));
    let sql = format!(
        "SELECT t.id, t.title, t.status, t.last_activity_at, t.bot_id,
                (SELECT provider FROM channel_conversation_bindings c WHERE c.thread_id = t.id AND c.owner = t.user_id LIMIT 1) AS channel
         FROM bot_threads t
         WHERE t.user_id = ?2 AND {VISIBLE}
           AND (?3 IS NULL OR t.title LIKE ?3 OR IFNULL(t.objective, '') LIKE ?3 OR IFNULL(t.summary, '') LIKE ?3)
           AND (?4 IS NULL OR (SELECT provider FROM channel_conversation_bindings c WHERE c.thread_id = t.id AND c.owner = t.user_id LIMIT 1) = ?4)
         ORDER BY t.last_activity_at DESC LIMIT 50"
    );
    let mut q = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let threads = q
        .query_map(params![s.vendor_bot_id, s.owner, query, channel], |r| {
            Ok(json!({
                "threadId": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?, "status": r.get::<_, String>(2)?,
                "lastActivityAt": r.get::<_, String>(3)?, "attached": r.get::<_, String>(4)? == s.vendor_bot_id,
                "channel": r.get::<_, Option<String>>(5)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(json!({ "threads": threads }))
}

pub fn read_thread(db: &DbHandle, s: &Session, args: &Value) -> Result<Value, String> {
    let id = str_arg(args, "threadId").ok_or("threadId is required.")?;
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(30).clamp(1, 100);
    let conn = db.connect().map_err(|e| e.to_string())?;
    let thread: Option<(String, String)> = conn
        .query_row(&format!("SELECT t.bot_id, t.title FROM bot_threads t WHERE t.id = ?3 AND t.user_id = ?2 AND {VISIBLE}"), params![s.vendor_bot_id, s.owner, id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    // Not visible reads exactly like not found.
    let Some((bot_id, title)) = thread else { return Err("That thread isn't one you can see.".into()) };
    let mut q = conn
        .prepare(
            "SELECT seq, event_type, actor_type, actor_id, payload, occurred_at FROM bot_events
             WHERE bot_id = ?1 AND thread_id = ?2 ORDER BY seq DESC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let mut messages: Vec<Value> = q
        .query_map(params![bot_id, id, limit * 3], |r| {
            let payload: Value = r.get::<_, String>(4).ok().and_then(|p| serde_json::from_str(&p).ok()).unwrap_or(Value::Null);
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, payload, r.get::<_, String>(5)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|(seq, ty, actor_type, actor_id, payload, at)| {
            let text = ["text", "summary", "transcript"].iter().find_map(|k| payload[k].as_str()).map(str::to_string);
            (text.is_some() || ty.starts_with("call.")).then(|| json!({ "seq": seq, "type": ty, "actor": { "type": actor_type, "id": actor_id }, "at": at, "text": text }))
        })
        .take(limit as usize)
        .collect();
    messages.reverse();
    Ok(json!({ "threadId": id, "title": title, "messages": messages }))
}

// ─── Dispatch, attribution and audit ───────────────────────────────────────────

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn args_hash(args: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(args).unwrap_or_default()))
}

fn audit(db: &DbHandle, s: &Session, tool: &str, args: &Value, outcome: &Result<Value, String>) {
    let (ok, result) = match outcome {
        Ok(v) => (1, v.to_string()),
        Err(e) => (0, e.clone()),
    };
    let result: String = result.chars().take(500).collect();
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "INSERT INTO vendor_bot_audit (owner, vendor_bot_id, directing_bot_id, client, tool, args_hash, ok, result, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![s.owner, s.vendor_bot_id, s.directing_bot_id, s.client, tool, args_hash(args), ok, result, chrono::Utc::now().to_rfc3339()],
        );
    }
}

/// Run one tool and answer with a complete MCP `tools/call` result. Failures
/// are `isError` results with a plain sentence, never JSON-RPC errors.
pub async fn call_tool(db: &DbHandle, actions: &dyn Actions, s: &Session, name: &str, args: Value) -> Value {
    let text = |k: &str| str_arg(&args, k).map(str::to_string);
    let need = |k: &str| text(k).ok_or_else(|| format!("{k} is required."));
    let outcome: Result<Value, String> = match name {
        "list_threads" => list_threads(db, s, &args),
        "read_thread" => read_thread(db, s, &args),
        "get_ticket" => crate::vendor_tickets::tool_get_ticket(db, &s.owner, &s.vendor_bot_id, &args),
        "post_result" => crate::vendor_tickets::tool_post_result(db, &s.owner, &s.vendor_bot_id, &args),
        "list_open_tickets" => crate::vendor_tickets::tool_list_open_tickets(db, &s.owner, &s.vendor_bot_id),
        "send_text" | "start_call" | "send_email" | "post_message" | "ask_bot" => {
            let long = ["text", "body", "purpose"].iter().any(|k| args[k].as_str().is_some_and(|v| v.chars().count() > MAX_TEXT_CHARS));
            if long {
                Err("That's too long. Shorten it and try again.".into())
            } else {
                match name {
                    "send_text" => match (need("to"), need("text")) {
                        (Ok(to), Ok(t)) => actions.send_text(s, &to, &t).await,
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    },
                    "start_call" => match (need("to"), need("purpose")) {
                        (Ok(to), Ok(p)) => actions.start_call(s, &to, &p).await,
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    },
                    "send_email" => match (need("to"), need("subject"), need("body")) {
                        (Ok(to), Ok(sub), Ok(b)) => actions.send_email(s, &to, &sub, &b).await,
                        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Err(e),
                    },
                    "post_message" => match (need("provider"), args.get("target").filter(|t| t.is_object()), need("text")) {
                        (Ok(p), Some(target), Ok(t)) => actions.post_message(s, &p.to_lowercase(), target, &t).await,
                        (Err(e), _, _) | (_, _, Err(e)) => Err(e),
                        (_, None, _) => Err("target is required.".into()),
                    },
                    _ => match need("text") {
                        Ok(t) => actions.ask_bot(s, &t).await,
                        Err(e) => Err(e),
                    },
                }
            }
        }
        other => Err(format!("Unknown tool: {other}")),
    };
    audit(db, s, name, &args, &outcome);
    // Attribution: a thread this call read or wrote is one the vendor bot took part in.
    if let Ok(v) = &outcome {
        if let Some(thread) = str_arg(&args, "threadId").or_else(|| v["threadId"].as_str()) {
            crate::vendor_tickets::record_participation(db, &s.owner, &s.vendor_bot_id, thread, &format!("tool:{name}"));
        }
    }
    let card = crate::mcp_vendor_cards::card_uri(name);
    let mut result = match &outcome {
        Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": false }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    };
    if let Some(uri) = card {
        result["structuredContent"] = crate::mcp_vendor_cards::structured(name, &args, &outcome);
        result["_meta"] = json!({ "ui": { "resourceUri": uri } });
    }
    result
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_err(id: Value, code: i32, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The JSON-RPC core, separate from the HTTP shell so tests drive it directly.
pub async fn handle_rpc(db: &DbHandle, actions: &dyn Actions, s: &Session, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned()?; // a notification gets no body
    let method = req["method"].as_str().unwrap_or_default();
    Some(match method {
        "initialize" => {
            let requested = req["params"]["protocolVersion"].as_str();
            let version = requested.filter(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSION);
            rpc_ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": { "subscribe": false, "listChanged": false } },
                    "serverInfo": { "name": "allternit-vendor-bot", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": SERVER_INSTRUCTIONS
                }),
            )
        }
        "ping" => rpc_ok(id, json!({})),
        "tools/list" => rpc_ok(id, json!({ "tools": tool_descriptors() })),
        "resources/list" => rpc_ok(id, json!({ "resources": crate::mcp_vendor_cards::resource_descriptors() })),
        "resources/read" => {
            let uri = req["params"]["uri"].as_str().unwrap_or_default();
            match crate::mcp_vendor_cards::read_resource(uri) {
                Some(result) => rpc_ok(id, result),
                None => rpc_err(id, -32002, format!("Resource not found: {uri}")),
            }
        }
        "resources/templates/list" => rpc_ok(id, json!({ "resourceTemplates": [] })),
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or_default();
            if !is_tool(name) {
                return Some(rpc_err(id, -32602, format!("Unknown tool: {name}")));
            }
            let args = req["params"].get("arguments").cloned().unwrap_or_else(|| json!({}));
            rpc_ok(id, call_tool(db, actions, s, name, args).await)
        }
        other => rpc_err(id, -32601, format!("Method not found: {other}")),
    })
}

// ─── OAuth clients ─────────────────────────────────────────────────────────────

fn client_label(claims: &Value) -> String {
    ["azp", "client_id", "cid"].iter().find_map(|k| claims[k].as_str()).filter(|c| !c.is_empty()).unwrap_or("oauth-client").to_string()
}

/// Record that `client` used the connector. `false` = the owner revoked it.
pub fn touch_client(db: &DbHandle, owner: &str, vendor_bot_id: &str, client: &str) -> bool {
    let Ok(conn) = db.connect() else { return false };
    let now = chrono::Utc::now().to_rfc3339();
    let _ = conn.execute(
        "INSERT INTO vendor_connector_clients (id, vendor_bot_id, owner, client, first_seen_at, last_used_at) VALUES (?1,?2,?3,?4,?5,?5)
         ON CONFLICT(vendor_bot_id, client) DO UPDATE SET last_used_at = excluded.last_used_at WHERE revoked_at IS NULL",
        params![format!("vcc_{}", uuid::Uuid::new_v4().simple()), vendor_bot_id, owner, client, now],
    );
    conn.query_row("SELECT revoked_at IS NULL FROM vendor_connector_clients WHERE vendor_bot_id = ?1 AND client = ?2", params![vendor_bot_id, client], |r| r.get::<_, bool>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(false)
}

fn challenge_response(vendor_bot_id: &str, status: StatusCode, body: Value, error: (&str, &str)) -> Response {
    let mut resp = (status, Json(body)).into_response();
    resp.headers_mut().insert(header::WWW_AUTHENTICATE, challenge_value_for(&bot_resource_url(vendor_bot_id), Some(error)));
    resp
}

/// `Ok(Some(client))` for an OAuth caller (token verified for this bot's URL and
/// `bots:act`), `Ok(None)` for the owner's own session, `Err` = a ready response.
pub async fn authorize_bot_bearer(state: &AppState, headers: &HeaderMap, user: &AuthUser, vendor_bot_id: &str) -> Result<Option<String>, Response> {
    let Some(token) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim) else {
        return Ok(None);
    };
    let Some(claims) = peek_claims(token) else { return Ok(None) };
    let issuer = claims.get("iss").and_then(Value::as_str).unwrap_or("");
    if claims.get("sid").is_some() || !crate::auth::is_clerk_issuer(issuer, &state.auth_config.clerk_issuer) {
        return Ok(None);
    }
    match crate::mcp_agents::verify_oauth_claims(&state.jwks, token, &state.auth_config, &bot_resource_url(vendor_bot_id), BOT_SCOPE).await {
        Ok((verified, claims)) if verified.user_id == user.user_id => Ok(Some(client_label(&claims))),
        Ok(_) => Err(challenge_response(vendor_bot_id, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token" }), ("invalid_token", "The access token is invalid"))),
        Err(McpTokenError::Invalid(msg)) => Err(challenge_response(vendor_bot_id, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token", "message": msg }), ("invalid_token", "The access token is invalid"))),
        Err(McpTokenError::InsufficientScope) => Err(challenge_response(vendor_bot_id, StatusCode::FORBIDDEN, json!({ "error": "insufficient_scope", "scope": BOT_SCOPE }), ("insufficient_scope", "bots:act is required"))),
    }
}

// ─── HTTP ──────────────────────────────────────────────────────────────────────

/// `POST /mcp/bots/:vendorBotId`, mounted inside the `/mcp` nest next to `/server`.
pub fn mcp_bots_router() -> Router<Arc<AppState>> {
    Router::new().route("/bots/:vendor_bot_id", post(handle_bot_rpc))
}

async fn handle_bot_rpc(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(vendor_bot_id): Path<String>,
    credential: Option<Extension<crate::enterprise_auth::CredentialContext>>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Response {
    let client = match authorize_bot_bearer(&state, &headers, &user, &vendor_bot_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // A scoped local-connector key counts as a client, so revoking it works like revoking an OAuth client.
    let client = match credential {
        Some(Extension(c)) => {
            if !c.allows(&crate::vendor_local_connector::key_scope(&vendor_bot_id)) {
                return (StatusCode::FORBIDDEN, Json(json!({ "error": "insufficient_scope" }))).into_response();
            }
            Some(format!("local-key:{}", c.credential_id))
        }
        None => client,
    };
    serve_bot_rpc(&state, &user.user_id, &vendor_bot_id, client, &req).await
}

/// One connector call for an already-authorized `owner`: the session, the
/// revoked-client check, then the JSON-RPC dispatch. Shared by the direct route
/// above and the cloud edge relay ([`crate::mcp_edge_relay`]), which verified
/// the OAuth token itself.
pub async fn serve_bot_rpc(state: &Arc<AppState>, owner: &str, vendor_bot_id: &str, client: Option<String>, req: &Value) -> Response {
    let Some(mut session) = load_session(&state.db, owner, vendor_bot_id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    };
    if let Some(client) = client {
        if !touch_client(&state.db, owner, vendor_bot_id, &client) {
            return challenge_response(vendor_bot_id, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token", "message": "This connection was revoked" }), ("invalid_token", "This connection was revoked"));
        }
        session.client = Some(client);
    }
    match handle_rpc(&state.db, &LiveActions::production(state), &session, req).await {
        Some(body) => Json(body).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Keys-page routes, mounted under `/api`: `/v1/vendor-bots/:id/connector*`.
pub fn connector_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/vendor-bots/:id/connector", get(connector_get).put(connector_put))
        .route("/v1/vendor-bots/:id/connector/clients/:client_id", delete(client_revoke))
        .route("/v1/vendor-bots/:id/connector/audit", get(audit_list))
        .route("/v1/vendor-bots/:id/connector/threads/:thread_id", put(thread_share).delete(thread_unshare))
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
}

fn owned(state: &AppState, user: &AuthUser, id: &str) -> Option<Session> {
    load_session(&state.db, &user.user_id, id)
}

pub fn connector_view(db: &DbHandle, s: &Session) -> Result<Value, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut q = conn
        .prepare("SELECT id, client, first_seen_at, last_used_at FROM vendor_connector_clients WHERE vendor_bot_id = ?1 AND owner = ?2 AND revoked_at IS NULL ORDER BY last_used_at DESC")
        .map_err(|e| e.to_string())?;
    let clients = q
        .query_map(params![s.vendor_bot_id, s.owner], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "client": r.get::<_, String>(1)?, "firstSeenAt": r.get::<_, String>(2)?, "lastUsedAt": r.get::<_, String>(3)? })))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut q = conn.prepare("SELECT thread_id FROM vendor_bot_thread_shares WHERE vendor_bot_id = ?1 AND owner = ?2 ORDER BY created_at").map_err(|e| e.to_string())?;
    let shared = q.query_map(params![s.vendor_bot_id, s.owner], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok(json!({
        "url": bot_resource_url(&s.vendor_bot_id),
        "metadataUrl": resource_metadata_url(&bot_resource_url(&s.vendor_bot_id)),
        "scopes": [BOT_SCOPE],
        "directingBotId": s.directing_bot_id,
        "directingBotName": s.directing_name,
        "sharedThreadIds": shared,
        "connectedClients": clients,
    }))
}

async fn connector_get(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let Some(s) = owned(&state, &user, &id) else { return not_found() };
    match connector_view(&state.db, &s) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectingBody {
    directing_bot_id: Option<String>,
}

async fn connector_put(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<DirectingBody>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    let directing = b.directing_bot_id.filter(|d| !d.trim().is_empty());
    if let Some(d) = &directing {
        let Ok(conn) = state.db.connect() else { return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "database unavailable" }))).into_response() };
        let ok: bool = conn.query_row("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2 AND id <> ?3", params![d, user.user_id, id], |_| Ok(true)).optional().ok().flatten().unwrap_or(false);
        if !ok {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "directingBotId must be one of your own bots, other than this vendor bot" }))).into_response();
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    let res = state.db.connect().and_then(|c| {
        c.execute(
            "INSERT INTO vendor_bot_connectors (vendor_bot_id, owner, directing_bot_id, created_at, updated_at) VALUES (?1,?2,?3,?4,?4)
             ON CONFLICT(vendor_bot_id) DO UPDATE SET directing_bot_id = excluded.directing_bot_id, updated_at = excluded.updated_at WHERE owner = excluded.owner",
            params![id, user.user_id, directing, now],
        )
        .map_err(Into::into)
    });
    if let Err(e) = res {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }
    connector_get(State(state), Extension(user), Path(id)).await
}

async fn client_revoke(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((id, client_id)): Path<(String, String)>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    let n = state
        .db
        .connect()
        .ok()
        .and_then(|c| {
            c.execute(
                "UPDATE vendor_connector_clients SET revoked_at = ?1 WHERE id = ?2 AND vendor_bot_id = ?3 AND owner = ?4 AND revoked_at IS NULL",
                params![chrono::Utc::now().to_rfc3339(), client_id, id, user.user_id],
            )
            .ok()
        })
        .unwrap_or(0);
    if n == 0 {
        return not_found();
    }
    Json(json!({ "ok": true })).into_response()
}

async fn audit_list(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    let rows = (|| -> Result<Vec<Value>, String> {
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        let mut q = conn
            .prepare("SELECT id, client, tool, args_hash, ok, result, created_at FROM vendor_bot_audit WHERE vendor_bot_id = ?1 AND owner = ?2 ORDER BY id DESC LIMIT 100")
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map(params![id, user.user_id], |r| {
                Ok(json!({ "id": r.get::<_, i64>(0)?, "client": r.get::<_, Option<String>>(1)?, "tool": r.get::<_, String>(2)?, "argsHash": r.get::<_, String>(3)?, "ok": r.get::<_, i64>(4)? == 1, "result": r.get::<_, String>(5)?, "at": r.get::<_, String>(6)? }))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    })();
    match rows {
        Ok(rows) => Json(json!({ "audit": rows })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

async fn thread_share(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((id, thread_id)): Path<(String, String)>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    let Ok(conn) = state.db.connect() else { return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "database unavailable" }))).into_response() };
    let mine: bool = conn.query_row("SELECT 1 FROM bot_threads WHERE id = ?1 AND user_id = ?2", params![thread_id, user.user_id], |_| Ok(true)).optional().ok().flatten().unwrap_or(false);
    if !mine {
        return not_found();
    }
    let _ = conn.execute("INSERT OR IGNORE INTO vendor_bot_thread_shares (vendor_bot_id, thread_id, owner, created_at) VALUES (?1,?2,?3,?4)", params![id, thread_id, user.user_id, chrono::Utc::now().to_rfc3339()]);
    Json(json!({ "ok": true })).into_response()
}

async fn thread_unshare(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((id, thread_id)): Path<(String, String)>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    if let Ok(conn) = state.db.connect() {
        let _ = conn.execute("DELETE FROM vendor_bot_thread_shares WHERE vendor_bot_id = ?1 AND thread_id = ?2 AND owner = ?3", params![id, thread_id, user.user_id]);
    }
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{test_clerk_token_with_claims, CLERK_PROXY_ISSUER};
    use crate::channel_phone::phone_key;
    use crate::channel_transports::{HttpReq, HttpResp};
    use std::sync::Mutex;

    const NUMBER: &str = "+14155550100";
    const CALLER: &str = "+14155550123";

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

    /// Answers every cloud call with `(status, body)` and remembers the requests.
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        reply: Mutex<(u16, Value)>,
    }
    impl FakeHttp {
        fn answering(status: u16, body: Value) -> Arc<Self> {
            Arc::new(Self { sent: Mutex::new(vec![]), reply: Mutex::new((status, body)) })
        }
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            let (status, body) = self.reply.lock().unwrap().clone();
            Ok(HttpResp { status, body })
        }
    }

    /// user-a: bot-native (Native, with a phone number and a sealed sms connection),
    /// bot-vendor (Vendor, a vendor binding); a thread each.
    async fn setup(tag: &str) -> Arc<AppState> {
        let st = crate::aai_facade::test_util::setup(tag, "READY").await;
        let secret = json!({ "token": "tok-a", "numberId": "num-1", "publicKey": "k", "cloudUrl": "https://cloud.test" }).to_string();
        let c = st.db.connect().unwrap();
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, secret_ref, state) VALUES ('acct-1','user-a','sms','channel_oauth','num-1',?1,'CONNECTED')",
            params![crate::token_crypto::seal(&secret)],
        )
        .unwrap();
        c.execute("INSERT INTO channel_phone_numbers (number_id, owner, bot_id, e164, account_id) VALUES ('num-1','user-a','bot-native',?1,'acct-1')", params![NUMBER]).unwrap();
        st
    }

    fn live(st: &Arc<AppState>, http: Arc<FakeHttp>) -> LiveActions<Rt> {
        LiveActions { state: st.clone(), rt: Rt, http, starter: None }
    }

    fn session(st: &AppState) -> Session {
        let mut s = load_session(&st.db, "user-a", "bot-vendor").expect("owner sees the vendor bot");
        s.client = Some("claude-connector".into());
        s
    }

    fn audit_rows(st: &AppState) -> Vec<(String, i64, String, String)> {
        let c = st.db.connect().unwrap();
        let mut q = c.prepare("SELECT tool, ok, args_hash, result FROM vendor_bot_audit ORDER BY id").unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
    }

    // ── access ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn only_the_owner_gets_a_session_and_only_for_a_vendor_bot() {
        let st = setup("own").await;
        let s = session(&st);
        assert_eq!((s.vendor_name.as_str(), s.directing_bot_id.as_deref(), s.directing_name.as_deref()), ("Vendor", Some("bot-native"), Some("Native")));
        assert!(load_session(&st.db, "user-b", "bot-vendor").is_none(), "someone else's vendor bot");
        assert!(load_session(&st.db, "user-a", "bot-native").is_none(), "a native bot has no vendor binding");
        assert!(load_session(&st.db, "user-a", "nope").is_none());
    }

    fn token_claims(aud: &str, scope: &str) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({ "iss": CLERK_PROXY_ISSUER, "sub": "user-a", "aud": aud, "scope": scope, "azp": "claude-connector", "iat": now, "exp": now + 300 })
    }

    async fn authorize(st: &Arc<AppState>, claims: Value, user: &str, bot: &str) -> Result<Option<String>, StatusCode> {
        let token = test_clerk_token_with_claims(&st.jwks, claims).await;
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        let u = AuthUser { user_id: user.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None };
        authorize_bot_bearer(st, &h, &u, bot).await.map_err(|r| {
            assert!(r.headers().contains_key(header::WWW_AUTHENTICATE), "a refusal carries the OAuth challenge");
            r.status()
        })
    }

    #[tokio::test]
    async fn oauth_tokens_must_be_for_this_bot_and_this_scope() {
        let st = setup("oauth").await;
        let url = bot_resource_url("bot-vendor");
        assert_eq!(url, "https://mcp.allternit.com/mcp/bots/bot-vendor");
        assert_eq!(authorize(&st, token_claims(&url, "openid bots:act"), "user-a", "bot-vendor").await, Ok(Some("claude-connector".into())));
        // the agents server's token, another bot's token, no scope, a stranger's token
        assert_eq!(authorize(&st, token_claims("https://mcp.allternit.com/mcp", "agents:read bots:act"), "user-a", "bot-vendor").await, Err(StatusCode::UNAUTHORIZED));
        assert_eq!(authorize(&st, token_claims(&bot_resource_url("other-bot"), "bots:act"), "user-a", "bot-vendor").await, Err(StatusCode::UNAUTHORIZED));
        assert_eq!(authorize(&st, token_claims(&url, "agents:read"), "user-a", "bot-vendor").await, Err(StatusCode::FORBIDDEN));
        assert_eq!(authorize(&st, token_claims(&url, "bots:act"), "user-b", "bot-vendor").await, Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn metadata_is_per_bot_with_its_own_scope() {
        assert_eq!(bot_id_from_resource_path("mcp/bots/abc"), Some("abc"));
        assert_eq!(bot_id_from_resource_path("/bots/abc/"), Some("abc"));
        assert_eq!(bot_id_from_resource_path("mcp"), None);
        assert_eq!(bot_id_from_resource_path("mcp/bots/a/b"), None);
        let doc = bot_protected_resource_metadata("abc");
        assert_eq!((doc["resource"].as_str(), doc["scopes_supported"].clone()), (Some("https://mcp.allternit.com/mcp/bots/abc"), json!(["bots:act"])));
        assert_eq!(resource_metadata_url(&bot_resource_url("abc")), "https://mcp.allternit.com/.well-known/oauth-protected-resource/mcp/bots/abc");
    }

    #[tokio::test]
    async fn a_revoked_client_is_refused_and_stays_refused() {
        let st = setup("revoke").await;
        assert!(touch_client(&st.db, "user-a", "bot-vendor", "claude-connector"));
        assert!(touch_client(&st.db, "user-a", "bot-vendor", "chatgpt"));
        let view = connector_view(&st.db, &session(&st)).unwrap();
        assert_eq!(view["connectedClients"].as_array().unwrap().len(), 2);
        assert_eq!(view["url"], "https://mcp.allternit.com/mcp/bots/bot-vendor");
        assert_eq!(view["scopes"], json!(["bots:act"]));
        let id = view["connectedClients"].as_array().unwrap().iter().find(|c| c["client"] == "chatgpt").unwrap()["id"].as_str().unwrap().to_string();
        let n = st.db.connect().unwrap().execute("UPDATE vendor_connector_clients SET revoked_at = 'now' WHERE id = ?1", params![id]).unwrap();
        assert_eq!(n, 1);
        assert!(!touch_client(&st.db, "user-a", "bot-vendor", "chatgpt"));
        assert!(!touch_client(&st.db, "user-a", "bot-vendor", "chatgpt"), "touching again does not undo a revoke");
        assert!(touch_client(&st.db, "user-a", "bot-vendor", "claude-connector"), "other clients are unaffected");
        let view = connector_view(&st.db, &session(&st)).unwrap();
        assert_eq!(view["connectedClients"].as_array().unwrap().len(), 1);
    }

    // ── their view ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn threads_are_only_the_ones_attached_or_shared() {
        let st = setup("view").await;
        let s = session(&st);
        let ids = |v: Value| -> Vec<String> { v["threads"].as_array().unwrap().iter().map(|t| t["threadId"].as_str().unwrap().to_string()).collect() };
        assert_eq!(ids(list_threads(&st.db, &s, &json!({})).unwrap()), ["th-vendor"], "the directing bot's thread is not theirs");
        // The directing bot's thread is theirs once shared.
        st.db.connect().unwrap().execute("INSERT INTO vendor_bot_thread_shares (vendor_bot_id, thread_id, owner) VALUES ('bot-vendor','th-native','user-a')", []).unwrap();
        let mut both = ids(list_threads(&st.db, &s, &json!({})).unwrap());
        both.sort();
        assert_eq!(both, ["th-native", "th-vendor"]);
        // A stranger's thread can't be shared into someone's view.
        st.db.connect().unwrap().execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES ('th-b','user-b','bot-native','Theirs','idle','2026','2026','2026')", []).unwrap();
        st.db.connect().unwrap().execute("INSERT INTO vendor_bot_thread_shares (vendor_bot_id, thread_id, owner) VALUES ('bot-vendor','th-b','user-a')", []).unwrap();
        assert!(!ids(list_threads(&st.db, &s, &json!({})).unwrap()).contains(&"th-b".to_string()));
        // query and channel filters
        assert_eq!(ids(list_threads(&st.db, &s, &json!({ "query": "zzz" })).unwrap()), Vec::<String>::new());
        let c = st.db.connect().unwrap();
        c.execute(
            "INSERT INTO channel_conversation_bindings (id, owner, thread_id, provider, external_conversation_id, sync_state, created_at, updated_at) VALUES ('ccb-1','user-a','th-native','sms','phone:x:y','LIVE','2026','2026')",
            [],
        )
        .unwrap();
        assert_eq!(ids(list_threads(&st.db, &s, &json!({ "channel": "SMS" })).unwrap()), ["th-native"]);
        assert_eq!(ids(list_threads(&st.db, &s, &json!({ "channel": "slack" })).unwrap()), Vec::<String>::new());
    }

    #[tokio::test]
    async fn reading_a_thread_outside_their_view_reads_like_not_found() {
        let st = setup("read").await;
        let s = session(&st);
        crate::gateway_runner::led(&st.db, "bot-vendor", "th-vendor", None, "channel.message.received", ("user", "u1"), json!({ "text": "hello there" }), None);
        crate::gateway_runner::led(&st.db, "bot-vendor", "th-vendor", None, "tool.noise", ("bot", "bot-vendor"), json!({ "x": 1 }), None);
        crate::gateway_runner::led(&st.db, "bot-native", "th-native", None, "channel.message.received", ("user", "u1"), json!({ "text": "private" }), None);
        let r = read_thread(&st.db, &s, &json!({ "threadId": "th-vendor" })).unwrap();
        let texts: Vec<_> = r["messages"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(texts, ["hello there"], "events without text are left out");
        assert_eq!(read_thread(&st.db, &s, &json!({ "threadId": "th-native" })).unwrap_err(), "That thread isn't one you can see.");
        assert_eq!(read_thread(&st.db, &s, &json!({ "threadId": "missing" })).unwrap_err(), "That thread isn't one you can see.");
        st.db.connect().unwrap().execute("INSERT INTO vendor_bot_thread_shares (vendor_bot_id, thread_id, owner) VALUES ('bot-vendor','th-native','user-a')", []).unwrap();
        assert_eq!(read_thread(&st.db, &s, &json!({ "threadId": "th-native" })).unwrap()["messages"][0]["text"], "private");
    }

    // ── the gates: the same path the bot takes ────────────────────────────────

    #[tokio::test]
    async fn send_text_goes_through_the_cloud_send_route_as_the_directing_bot_and_says_who_sent_it() {
        let st = setup("sms").await;
        let http = FakeHttp::answering(200, json!({ "messageId": "m1", "status": "queued" }));
        let out = live(&st, http.clone()).send_text(&session(&st), CALLER, "Your table is ready").await.unwrap();
        assert_eq!((out["sent"].clone(), out["messageId"].clone(), out["from"].clone()), (json!(true), json!("m1"), json!(NUMBER)));
        let req = http.sent.lock().unwrap()[0].clone();
        assert_eq!(req.url, "https://cloud.test/api/v1/channels/sms/send");
        assert_eq!(req.headers, vec![("Authorization".to_string(), "Bearer tok-a".to_string())]);
        assert_eq!((req.body["numberId"].as_str(), req.body["to"].as_str()), (Some("num-1"), Some(CALLER)));
        let text = req.body["text"].as_str().unwrap();
        assert!(text.starts_with("Your table is ready") && text.contains("Sent by Vendor via Native"), "{text}");
        // it is a message in the phone thread the bot's own texts use
        let c = st.db.connect().unwrap();
        let key: String = c.query_row("SELECT external_conversation_id FROM channel_conversation_bindings WHERE provider = 'sms'", [], |r| r.get(0)).unwrap();
        assert_eq!(key, phone_key(NUMBER, CALLER));
    }

    #[tokio::test]
    async fn each_cloud_refusal_on_a_text_comes_back_as_one_plain_sentence() {
        let st = setup("sms-no").await;
        for (status, slug, expect) in [
            (403, "no_consent", "hasn't texted or called this number"),
            (403, "recipient_opted_out", "replied STOP"),
            (403, "sms_not_active", "carrier registration"),
            (429, "daily_limit", "daily limit"),
        ] {
            let http = FakeHttp::answering(status, json!({ "error": slug }));
            let e = live(&st, http.clone()).send_text(&session(&st), CALLER, "hi").await.unwrap_err();
            assert!(e.contains(expect), "{slug}: {e}");
            assert_eq!(http.sent.lock().unwrap().len(), 1, "the cloud was asked exactly once; nothing bypasses it");
        }
        let http = FakeHttp::answering(200, json!({}));
        assert!(live(&st, http.clone()).send_text(&session(&st), "415-555-0123", "hi").await.unwrap_err().contains("+14155550123"));
        assert!(http.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn start_call_asks_the_cloud_consent_gate_and_says_who_asked() {
        let st = setup("call").await;
        let http = FakeHttp::answering(200, json!({ "consentRef": "c1", "room": "room-9", "dialing": true }));
        let out = live(&st, http.clone()).start_call(&session(&st), CALLER, "Confirm Friday's booking").await.unwrap();
        assert_eq!((out["calling"].clone(), out["room"].clone()), (json!(true), json!("room-9")));
        let req = http.sent.lock().unwrap()[0].clone();
        assert_eq!(req.url, "https://cloud.test/api/v1/phone/calls/outbound");
        assert_eq!((req.body["botId"].as_str(), req.body["numberId"].as_str()), (Some("bot-native"), Some("num-1")));
        let purpose = req.body["purpose"].as_str().unwrap();
        assert!(purpose.contains("Vendor via Native") && purpose.contains("Confirm Friday's booking"), "{purpose}");

        let no = FakeHttp::answering(403, json!({ "error": "no_consent" }));
        let e = live(&st, no).start_call(&session(&st), CALLER, "x").await.unwrap_err();
        assert!(e.contains("hasn't texted or called this number"), "{e}");
        let off = FakeHttp::answering(200, json!({ "consentRef": "c1" }));
        assert_eq!(live(&st, off).start_call(&session(&st), CALLER, "x").await.unwrap_err(), "Outbound calling isn't switched on yet.");
    }

    #[tokio::test]
    async fn email_is_only_ever_the_approval_gated_path() {
        let st = setup("mail").await;
        let http = FakeHttp::answering(200, json!({}));
        // Mailflare isn't configured here, so the approval-gated path refuses in a sentence; there is no other path.
        let e = live(&st, http).send_email(&session(&st), "a@b.co", "Hi", "Body").await.unwrap_err();
        assert_eq!(e, "Email isn't switched on for this computer yet.");
    }

    #[tokio::test]
    async fn post_message_says_so_until_the_channel_start_path_is_wired() {
        let st = setup("post").await;
        let e = live(&st, FakeHttp::answering(200, json!({}))).post_message(&session(&st), "slack", &json!({ "kind": "user", "id": "U1" }), "hi").await.unwrap_err();
        assert_eq!(e, "Starting a conversation on a channel isn't available yet.");

        struct Starter(Mutex<Vec<(String, String, String, Value, String)>>);
        #[async_trait]
        impl ChannelStarter for Starter {
            async fn start(&self, owner: &str, bot_id: &str, provider: &str, target: &Value, text: &str) -> Result<Value, String> {
                self.0.lock().unwrap().push((owner.into(), bot_id.into(), provider.into(), target.clone(), text.into()));
                Ok(json!({ "threadId": "t1" }))
            }
        }
        let starter = Arc::new(Starter(Mutex::new(vec![])));
        let mut l = live(&st, FakeHttp::answering(200, json!({})));
        l.starter = Some(starter.clone());
        l.post_message(&session(&st), "slack", &json!({ "kind": "user", "id": "U1" }), "hi").await.unwrap();
        let got = starter.0.lock().unwrap()[0].clone();
        assert_eq!((got.0.as_str(), got.1.as_str(), got.2.as_str()), ("user-a", "bot-native", "slack"));
        assert!(got.4.contains("Sent by Vendor via Native"));
    }

    #[tokio::test]
    async fn with_no_phone_or_no_directing_bot_a_tool_says_what_is_missing() {
        let st = setup("none").await;
        // Bot-native is chosen explicitly, then loses its number.
        st.db.connect().unwrap().execute("INSERT INTO vendor_bot_connectors (vendor_bot_id, owner, directing_bot_id) VALUES ('bot-vendor','user-a','bot-native')", []).unwrap();
        st.db.connect().unwrap().execute("DELETE FROM channel_phone_numbers", []).unwrap();
        let e = live(&st, FakeHttp::answering(200, json!({}))).send_text(&session(&st), CALLER, "hi").await.unwrap_err();
        assert_eq!(e, "Native doesn't have a phone number connected on this computer.");
        // a directing bot that isn't the owner's is never accepted
        st.db.connect().unwrap().execute("UPDATE vendor_bot_connectors SET directing_bot_id = 'bot-ghost'", []).unwrap();
        assert!(session(&st).directing_bot_id.is_none(), "a directing bot that isn't the owner's is ignored");
        let e = live(&st, FakeHttp::answering(200, json!({}))).send_text(&session(&st), CALLER, "hi").await.unwrap_err();
        assert!(e.contains("no directing bot"), "{e}");
    }

    // ── dispatch, attribution, audit ──────────────────────────────────────────

    #[derive(Default)]
    struct Fake(Mutex<Vec<String>>);
    #[async_trait]
    impl Actions for Fake {
        async fn send_text(&self, s: &Session, to: &str, text: &str) -> Result<Value, String> {
            self.0.lock().unwrap().push(format!("send_text {} {to} {text}", s.vendor_bot_id));
            if text == "refuse" { Err("nope".into()) } else { Ok(json!({ "sent": true })) }
        }
        async fn start_call(&self, _: &Session, to: &str, purpose: &str) -> Result<Value, String> {
            self.0.lock().unwrap().push(format!("start_call {to} {purpose}"));
            Ok(json!({}))
        }
        async fn send_email(&self, _: &Session, to: &str, subject: &str, body: &str) -> Result<Value, String> {
            self.0.lock().unwrap().push(format!("send_email {to} {subject} {body}"));
            Ok(json!({}))
        }
        async fn post_message(&self, _: &Session, p: &str, _: &Value, text: &str) -> Result<Value, String> {
            self.0.lock().unwrap().push(format!("post_message {p} {text}"));
            Ok(json!({}))
        }
        async fn ask_bot(&self, _: &Session, text: &str) -> Result<Value, String> {
            self.0.lock().unwrap().push(format!("ask_bot {text}"));
            Ok(json!({}))
        }
    }

    #[tokio::test]
    async fn every_tool_call_writes_an_audit_row_with_a_hash_not_the_arguments() {
        let st = setup("audit").await;
        let (s, fake) = (session(&st), Fake::default());
        let args = json!({ "to": CALLER, "text": "secret plans" });
        let ok = call_tool(&st.db, &fake, &s, "send_text", args.clone()).await;
        assert_eq!(ok["isError"], false);
        let bad = call_tool(&st.db, &fake, &s, "send_text", json!({ "to": CALLER, "text": "refuse" })).await;
        assert_eq!((bad["isError"].clone(), bad["content"][0]["text"].clone()), (json!(true), json!("nope")));
        let missing = call_tool(&st.db, &fake, &s, "send_text", json!({ "to": CALLER })).await;
        assert_eq!(missing["content"][0]["text"], "text is required.");
        for (tool, a) in [("start_call", json!({ "to": CALLER, "purpose": "p" })), ("send_email", json!({ "to": "a@b.co", "subject": "s", "body": "b" })), ("post_message", json!({ "provider": "Slack", "target": { "kind": "user" }, "text": "t" })), ("ask_bot", json!({ "text": "q" }))] {
            assert_eq!(call_tool(&st.db, &fake, &s, tool, a).await["isError"], false, "{tool}");
        }
        assert_eq!(call_tool(&st.db, &fake, &s, "post_message", json!({ "provider": "slack", "text": "t" })).await["content"][0]["text"], "target is required.");
        let rows = audit_rows(&st);
        assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["send_text", "send_text", "send_text", "start_call", "send_email", "post_message", "ask_bot", "post_message"]);
        assert_eq!(rows.iter().map(|r| r.1).collect::<Vec<_>>(), [1, 0, 0, 1, 1, 1, 1, 0]);
        assert_eq!(rows[0].2, args_hash(&args));
        assert!(rows.iter().all(|r| !r.2.contains("secret") && !r.3.contains("secret plans")));
        assert!(fake.0.lock().unwrap().contains(&"post_message slack t".to_string()), "the provider is normalised");
        let c = st.db.connect().unwrap();
        let (client, directing): (String, String) = c.query_row("SELECT client, directing_bot_id FROM vendor_bot_audit LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((client.as_str(), directing.as_str()), ("claude-connector", "bot-native"));
    }

    #[tokio::test]
    async fn rpc_lists_ten_tools_and_refuses_unknown_ones() {
        let st = setup("rpc").await;
        let (s, fake) = (session(&st), Fake::default());
        let init = handle_rpc(&st.db, &fake, &s, &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(SERVER_INSTRUCTIONS.len() < 512);
        let list = handle_rpc(&st.db, &fake, &s, &json!({ "id": 2, "method": "tools/list" })).await.unwrap();
        let names: Vec<_> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["list_threads", "read_thread", "send_text", "start_call", "send_email", "post_message", "ask_bot", "get_ticket", "post_result", "list_open_tickets"]);
        for t in list["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert_eq!(t["annotations"]["destructiveHint"], false);
        }
        let unknown = handle_rpc(&st.db, &fake, &s, &json!({ "id": 3, "method": "tools/call", "params": { "name": "shell.exec", "arguments": {} } })).await.unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
        assert!(handle_rpc(&st.db, &fake, &s, &json!({ "method": "notifications/initialized" })).await.is_none());
        assert_eq!(handle_rpc(&st.db, &fake, &s, &json!({ "id": 4, "method": "nope" })).await.unwrap()["error"]["code"], -32601);
        assert!(audit_rows(&st).is_empty(), "protocol calls are not tool calls");
    }

    #[tokio::test]
    async fn card_tools_name_their_ui_resource_and_answer_with_card_data_even_when_refused() {
        let st = setup("cards").await;
        let (s, fake) = (session(&st), Fake::default());
        let list = handle_rpc(&st.db, &fake, &s, &json!({ "id": 1, "method": "tools/list" })).await.unwrap();
        for t in list["result"]["tools"].as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            match crate::mcp_vendor_cards::card_uri(name) {
                Some(uri) => assert_eq!(t["_meta"]["ui"]["resourceUri"], uri, "{name}"),
                None => assert!(t.get("_meta").is_none(), "{name}"),
            }
        }
        let res = handle_rpc(&st.db, &fake, &s, &json!({ "id": 2, "method": "resources/list" })).await.unwrap();
        let uris: Vec<_> = res["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap().to_string()).collect();
        assert_eq!(uris.len(), 4);
        for uri in &uris {
            let read = handle_rpc(&st.db, &fake, &s, &json!({ "id": 3, "method": "resources/read", "params": { "uri": uri } })).await.unwrap();
            assert_eq!(read["result"]["contents"][0]["mimeType"], "text/html;profile=mcp-app");
        }
        let missing = handle_rpc(&st.db, &fake, &s, &json!({ "id": 4, "method": "resources/read", "params": { "uri": "ui://allternit/nope" } })).await.unwrap();
        assert_eq!(missing["error"]["code"], -32002);
        let ok = call_tool(&st.db, &fake, &s, "send_text", json!({ "to": "+14155550123", "text": "hello" })).await;
        assert_eq!(ok["structuredContent"]["request"]["text"], "hello");
        assert_eq!(ok["_meta"]["ui"]["resourceUri"], crate::mcp_vendor_cards::RESULT_URI);
        let no = call_tool(&st.db, &fake, &s, "send_text", json!({ "to": "+14155550123", "text": "refuse" })).await;
        assert_eq!(no["isError"], true);
        assert_eq!(no["content"][0]["text"], "nope");
        assert_eq!(no["structuredContent"]["refused"], true);
        let tickets = call_tool(&st.db, &fake, &s, "list_open_tickets", json!({})).await;
        assert!(tickets.get("_meta").is_none());
    }

    #[test]
    fn the_plain_instructions_name_every_tool_and_every_cli_command() {
        for t in tool_descriptors() {
            let name = t["name"].as_str().unwrap();
            assert!(BOT_INSTRUCTIONS.contains(name), "{name} missing from the instructions");
        }
        for cmd in ["threads", "read", "text", "call", "email", "post", "ask", "tickets", "ticket", "result", "help"] {
            assert!(BOT_INSTRUCTIONS.contains(&format!("allternit-bot {cmd}")), "{cmd}");
        }
    }

    #[tokio::test]
    async fn the_connector_route_is_in_the_router_without_overlapping_another() {
        // Building the routers panics on a duplicate path inside one router.
        let _ = mcp_bots_router();
        let _ = connector_router();
    }
}
