//! The one Allternit event registry, and the user-scoped emitter.
//!
//! Every event name any sink can see lives in [`REGISTRY`]: its description,
//! the JSON Schemas MCP `events/list` publishes (`inputSchema` = the filter
//! arguments a subscription may pass, `payloadSchema` = the `data` it gets),
//! and who may see it:
//!
//! * **Platform API webhooks** (`platform`): what a project endpoint may
//!   subscribe to. [`PLATFORM_EVENTS`] is the 051 list plus the P3 `call.*` events.
//! * **MCP agents server** (`agents`, scope `agents:read`, `/mcp`): agent,
//!   approval, thread, inbox, message, call, subscription and usage events.
//! * **MCP vendor-bot connector** (`bot`, scope `bots:act`, `/mcp/bots/:id`):
//!   that bot's thread, message and ticket events; `bot_id` is bound to the
//!   path's bot and never a caller-chosen argument.
//!
//! Names follow the runtime `bot_events` ledger where one exists; the ledger
//! aliases a runtime may forward are in [`EventType::runtime_aliases`] and
//! resolved by [`from_runtime`] (`routes::runtime_events`).
//!
//! Storage is the Platform API queue (`platform_v1::events`), generalised by
//! migration 057: [`emit_user_event`] stores a `subject = 'user'` event and
//! queues one delivery per matching live MCP subscription; the same worker
//! signs (Standard Webhooks) and retries it.

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use super::platform_v1::new_id;

/// Who can see an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// Platform API project webhooks.
    Platform,
    /// The MCP agents server (`agents:read`).
    Agents,
    /// The MCP vendor-bot connector (`bots:act`), bound to one bot.
    Bot,
}

/// One registered event.
#[derive(Debug)]
pub struct EventType {
    pub name: &'static str,
    /// Short plain-words label for people (Settings › Connected apps), e.g. "Approval requests".
    pub title: &'static str,
    pub description: &'static str,
    pub platform: bool,
    pub agents: bool,
    pub bot: bool,
    /// Optional string filters a subscription may pass; an event is delivered
    /// when its `data` carries every given key with the same value.
    pub filters: &'static [(&'static str, &'static str)],
    /// Fields `data` carries (all strings unless noted in the description).
    pub payload: &'static [(&'static str, &'static str)],
    /// Runtime `bot_events` types forwarded under this name.
    pub runtime_aliases: &'static [&'static str],
}

impl EventType {
    pub fn visible_to(&self, audience: Audience) -> bool {
        match audience {
            Audience::Platform => self.platform,
            Audience::Agents => self.agents,
            Audience::Bot => self.bot,
        }
    }

    /// `inputSchema` for `events/list`. The bot connector's `bot_id` is bound
    /// by the server, so it isn't offered there.
    pub fn input_schema(&self, audience: Audience) -> Value {
        let mut props = Map::new();
        for (key, desc) in self.filters {
            if audience == Audience::Bot && *key == "bot_id" {
                continue;
            }
            props.insert((*key).into(), json!({ "type": "string", "description": desc }));
        }
        json!({ "type": "object", "properties": props, "additionalProperties": false })
    }

    pub fn payload_schema(&self) -> Value {
        let mut props = Map::new();
        for (key, desc) in self.payload {
            props.insert((*key).into(), json!({ "description": desc }));
        }
        json!({ "type": "object", "properties": props, "additionalProperties": true })
    }

    pub fn filter_keys(&self, audience: Audience) -> impl Iterator<Item = &'static str> + '_ {
        self.filters.iter().map(|(k, _)| *k).filter(move |k| !(audience == Audience::Bot && *k == "bot_id"))
    }
}

const BOT: (&str, &str) = ("bot_id", "Only events for this bot (agent id).");
const THREAD: (&str, &str) = ("thread_id", "Only events in this thread.");

const P_BOT: (&str, &str) = ("bot_id", "The bot (agent) the event belongs to.");
const P_THREAD: (&str, &str) = ("thread_id", "The thread, when there is one.");

/// The registry. Order is the order `events/list` shows.
pub static REGISTRY: &[EventType] = &[
    EventType {
        name: "approval.requested",
        title: "Approval requests",
        description: "An agent is waiting for the owner to approve an action (a tool call, an outbound message, a payment).",
        platform: false,
        agents: true,
        bot: false,
        filters: &[BOT, THREAD],
        payload: &[P_BOT, P_THREAD, ("approval_id", "Approval id."), ("summary", "What the agent wants to do."), ("run_id", "The run waiting on it, when there is one.")],
        runtime_aliases: &["approval.requested", "agent.approval.requested"],
    },
    EventType {
        name: "approval.resolved",
        title: "Approval decisions",
        description: "An approval was granted or denied.",
        platform: false,
        agents: true,
        bot: false,
        filters: &[BOT, THREAD],
        payload: &[P_BOT, P_THREAD, ("approval_id", "Approval id."), ("decision", "approved | denied.")],
        runtime_aliases: &["approval.resolved", "agent.run.approval_resolved"],
    },
    EventType {
        name: "agent.run.completed",
        title: "Finished agent runs",
        description: "An agent run finished.",
        platform: false,
        agents: true,
        bot: false,
        filters: &[BOT],
        payload: &[P_BOT, P_THREAD, ("run_id", "Run id."), ("status", "completed | failed | cancelled.")],
        runtime_aliases: &["agent.run.completed", "run.completed"],
    },
    EventType {
        name: "thread.needs_user",
        title: "Threads that need you",
        description: "A bot's thread needs the owner (a question, a held outbound email, a blocked task).",
        platform: false,
        agents: true,
        bot: true,
        filters: &[BOT, THREAD],
        payload: &[P_BOT, P_THREAD, ("reason", "Why the owner is needed.")],
        runtime_aliases: &["thread.needs_user"],
    },
    EventType {
        name: "message.received",
        title: "New messages",
        description: "A message arrived. Platform API: an inbound text on a project number. Agents and bots: a new inbound message on one of the bot's channels.",
        platform: true,
        agents: true,
        bot: true,
        filters: &[BOT, THREAD, ("channel", "Only this channel (e.g. sms, email, telegram).")],
        payload: &[P_BOT, P_THREAD, ("channel", "Channel the message came in on."), ("from", "Sender, as the channel names them."), ("text", "Message text (may be truncated).")],
        runtime_aliases: &["message.received", "channel.message.received"],
    },
    EventType {
        name: "message.status",
        title: "Text delivery updates",
        description: "Platform API: an outbound text changed delivery status.",
        platform: true,
        agents: false,
        bot: false,
        filters: &[],
        payload: &[("id", "Message id."), ("status", "New status.")],
        runtime_aliases: &[],
    },
    EventType {
        name: "registration.updated",
        title: "Number registration changes",
        description: "Platform API: a number's 10DLC / toll-free registration changed state.",
        platform: true,
        agents: false,
        bot: false,
        filters: &[],
        payload: &[("number_id", "Number id."), ("state", "New registration state.")],
        runtime_aliases: &[],
    },
    EventType {
        name: "call.started",
        title: "Started calls",
        description: "Platform API: a call with a project's agent started (phone in or out, or a realtime session). data.call is the call.",
        platform: true,
        agents: false,
        bot: false,
        filters: &[],
        payload: &[("call", "The call object (object).")],
        runtime_aliases: &[],
    },
    EventType {
        name: "call.ended",
        title: "Ended calls",
        description: "A phone or in-app call with a bot ended. Platform API: data.call is the call, with status, duration_seconds and end_reason.",
        platform: true,
        agents: true,
        bot: false,
        filters: &[BOT],
        payload: &[P_BOT, P_THREAD, ("call_id", "Call id."), ("missed", "true when nobody answered (boolean)."), ("duration_s", "Seconds (number).")],
        runtime_aliases: &["call.ended"],
    },
    EventType {
        name: "call.transcript.ready",
        title: "Call transcripts",
        description: "Platform API: a call's transcript is complete (sent right after call.ended). data.transcript has every line.",
        platform: true,
        agents: false,
        bot: false,
        filters: &[],
        payload: &[("call_id", "Call id."), ("lines", "Number of lines (number)."), ("transcript", "The transcript object (object).")],
        runtime_aliases: &[],
    },
    EventType {
        name: "inbox.item.created",
        title: "New inbox items",
        description: "A new item landed in the owner's Allternit inbox.",
        platform: false,
        agents: true,
        bot: false,
        filters: &[BOT],
        payload: &[P_BOT, P_THREAD, ("item_id", "Inbox item id."), ("kind", "Item kind."), ("title", "Short title.")],
        runtime_aliases: &["inbox.item.created"],
    },
    EventType {
        name: "vendor.ticket.created",
        title: "New vendor tickets",
        description: "A ticket was opened for a vendor bot to work on (call get_ticket, do the work, then post_result).",
        platform: false,
        agents: false,
        bot: true,
        filters: &[BOT, THREAD],
        payload: &[P_BOT, P_THREAD, ("ticket_id", "Ticket id (for get_ticket)."), ("deadline_at", "RFC 3339 deadline.")],
        runtime_aliases: &["vendor.ticket.created"],
    },
    EventType {
        name: "subscription.login_needed",
        title: "AI subscription needs sign-in",
        description: "A connected AI subscription (e.g. ChatGPT, Claude) needs the owner to sign in again.",
        platform: false,
        agents: true,
        bot: false,
        filters: &[("provider", "Only this provider.")],
        payload: &[("provider", "Provider id."), ("account", "Account label, when known.")],
        runtime_aliases: &["subscription.login_needed"],
    },
    EventType {
        name: "subscription.signed_in",
        title: "AI subscription signed in again",
        description: "A connected AI subscription is signed in again.",
        platform: false,
        agents: true,
        bot: false,
        filters: &[("provider", "Only this provider.")],
        payload: &[("provider", "Provider id."), ("account", "Account label, when known.")],
        runtime_aliases: &["subscription.signed_in"],
    },
    EventType {
        name: "usage.threshold",
        title: "Usage alerts",
        description: "The owner's usage crossed a plan threshold (e.g. 80% or 100% of included minutes or credits).",
        platform: false,
        agents: true,
        bot: false,
        filters: &[("meter", "Only this meter.")],
        payload: &[("meter", "Meter name (`cloud_spend` = the monthly cloud budget, in USD)."), ("percent", "Threshold crossed (number)."), ("used", "Amount used (number)."), ("limit", "Plan amount (number)."), ("period", "Billing period, e.g. 2026-10.")],
        runtime_aliases: &["usage.threshold"],
    },
];

/// Event types a Platform API webhook can subscribe to (unchanged from 051).
pub const PLATFORM_EVENTS: [&str; 6] = ["message.received", "message.status", "registration.updated", "call.started", "call.ended", "call.transcript.ready"];

pub fn find(name: &str) -> Option<&'static EventType> {
    REGISTRY.iter().find(|e| e.name == name)
}

/// Every registered event the audience may see.
pub fn visible(audience: Audience) -> impl Iterator<Item = &'static EventType> {
    REGISTRY.iter().filter(move |e| e.visible_to(audience))
}

/// `events/list` entries for an audience.
pub fn mcp_list(audience: Audience) -> Vec<mcp_protocol::events::EventDef> {
    visible(audience)
        .filter(|e| e.agents || e.bot)
        .map(|e| mcp_protocol::events::EventDef {
            name: e.name,
            description: e.description,
            delivery: vec![mcp_protocol::events::DeliveryMode::Webhook],
            input_schema: e.input_schema(audience),
            payload_schema: e.payload_schema(),
        })
        .collect()
}

/// The registry name a runtime ledger type is forwarded as, or `None` when
/// it isn't an event any sink carries (it is then ignored, not an error).
/// Our own echoes (`channel.message.received` with `own: true`) are dropped.
pub fn from_runtime(ledger_type: &str, data: &Value) -> Option<&'static str> {
    if ledger_type == "channel.message.received" && data["own"].as_bool() == Some(true) {
        return None;
    }
    REGISTRY
        .iter()
        .find(|e| (e.agents || e.bot) && e.runtime_aliases.contains(&ledger_type))
        .map(|e| e.name)
}

/// Store a user-scoped event and queue it for every live, verified MCP
/// subscription of that user whose name matches and whose arguments the
/// event's `data` contains. `source`/`source_id` make the call idempotent:
/// `Ok(None)` = already stored under that pair.
pub async fn emit_user_event(
    db: &PgPool,
    user_id: &str,
    name: &str,
    data: &Value,
    occurred_at: Option<DateTime<Utc>>,
    source: &str,
    source_id: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let id = new_id("evt_");
    let mut tx = db.begin().await?;
    let stored: Option<String> = sqlx::query_scalar(
        "INSERT INTO platform_events (id, project_id, subject, user_id, type, data, source, source_id, occurred_at) \
         VALUES ($1, NULL, 'user', $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (source, source_id) WHERE source_id IS NOT NULL DO NOTHING RETURNING id",
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(data)
    .bind(source)
    .bind(source_id)
    .bind(occurred_at)
    .fetch_optional(&mut *tx)
    .await?;
    if stored.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    sqlx::query(
        "INSERT INTO platform_webhook_deliveries (id, webhook_id, event_id) \
         SELECT 'whd_' || replace(gen_random_uuid()::text, '-', ''), w.id, $2 FROM platform_webhooks w \
         WHERE w.kind = 'mcp_subscription' AND w.deleted_at IS NULL AND w.user_id = $1 AND $3 = ANY(w.events) \
           AND w.verified_at IS NOT NULL AND (w.refresh_before IS NULL OR w.refresh_before > now()) \
           AND $4::jsonb @> w.arguments \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(&id)
    .bind(name)
    .bind(data)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// The servers that offer an event, as `GET /api/v1/events/catalog` names them:
/// `agents` (the MCP agents server, `/mcp`), `bot` (a vendor-bot connector,
/// `/mcp/bots/:id`), `platform` (Platform API project webhooks).
pub fn servers(e: &EventType) -> Vec<&'static str> {
    [(e.agents, "agents"), (e.bot, "bot"), (e.platform, "platform")].into_iter().filter(|(on, _)| *on).map(|(_, s)| s).collect()
}

/// The registry as people see it (Settings › Connected apps).
pub fn catalog() -> Value {
    json!({
        "events": REGISTRY.iter().map(|e| json!({
            "name": e.name,
            "title": e.title,
            "description": e.description,
            "servers": servers(e),
        })).collect::<Vec<_>>()
    })
}

/// `GET /api/v1/events/catalog` (signed-in users): every registered event with
/// a plain-words title, its description and which servers offer it.
pub fn routes() -> axum::Router<std::sync::Arc<crate::ApiState>> {
    axum::Router::new().route("/api/v1/events/catalog", axum::routing::get(catalog_route))
}

async fn catalog_route(axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::ApiState>>, headers: axum::http::HeaderMap) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::auth::resolve_user(&state.db, &headers).await {
        return e.into_response();
    }
    axum::Json(catalog()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_platform_list_is_the_051_list() {
        let mut names: Vec<_> = REGISTRY.iter().map(|e| e.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), REGISTRY.len());
        let platform: Vec<_> = visible(Audience::Platform).map(|e| e.name).collect();
        assert_eq!(platform, PLATFORM_EVENTS.to_vec());
        assert_eq!(super::super::platform_v1::events::EVENT_TYPES, PLATFORM_EVENTS);
    }

    #[test]
    fn scopes_split_agents_and_bot() {
        let agents: Vec<_> = mcp_list(Audience::Agents).into_iter().map(|e| e.name).collect();
        let bot: Vec<_> = mcp_list(Audience::Bot).into_iter().map(|e| e.name).collect();
        assert!(agents.contains(&"approval.requested") && agents.contains(&"subscription.login_needed"));
        assert!(!agents.contains(&"vendor.ticket.created") && !agents.contains(&"message.status"));
        assert_eq!(bot, vec!["thread.needs_user", "message.received", "vendor.ticket.created"]);
        // The bot connector never offers a bot_id argument: it is bound to the path.
        for e in mcp_list(Audience::Bot) {
            assert!(e.input_schema["properties"].get("bot_id").is_none(), "{}", e.name);
        }
        let a = mcp_list(Audience::Agents).into_iter().find(|e| e.name == "approval.requested").unwrap();
        assert_eq!(a.input_schema["properties"]["bot_id"]["type"], "string");
        assert_eq!(a.input_schema["additionalProperties"], false);
    }

    #[test]
    fn runtime_ledger_names_map_onto_the_registry() {
        assert_eq!(from_runtime("channel.message.received", &json!({ "own": false })), Some("message.received"));
        assert_eq!(from_runtime("channel.message.received", &json!({ "own": true })), None);
        assert_eq!(from_runtime("agent.approval.requested", &json!({})), Some("approval.requested"));
        assert_eq!(from_runtime("run.completed", &json!({})), Some("agent.run.completed"));
        assert_eq!(from_runtime("call.ended", &json!({})), Some("call.ended"));
        assert_eq!(from_runtime("inbox.changed", &json!({})), None);
        // Platform-only events can't be injected by a runtime.
        assert_eq!(from_runtime("message.status", &json!({})), None);
        assert_eq!(from_runtime("registration.updated", &json!({})), None);
    }

    #[test]
    fn catalog_has_a_title_and_servers_for_every_event() {
        let c = catalog();
        let events = c["events"].as_array().unwrap();
        assert_eq!(events.len(), REGISTRY.len());
        for e in events {
            let title = e["title"].as_str().unwrap();
            assert!(!title.is_empty() && !title.contains('.'), "{e}");
            assert!(!e["servers"].as_array().unwrap().is_empty(), "{e}");
        }
        let approval = events.iter().find(|e| e["name"] == "approval.requested").unwrap();
        assert_eq!(approval["servers"], json!(["agents"]));
        let msg = events.iter().find(|e| e["name"] == "message.received").unwrap();
        assert_eq!(msg["servers"], json!(["agents", "bot", "platform"]));
    }
}
