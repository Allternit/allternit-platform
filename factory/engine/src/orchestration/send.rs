//! The one send path (`orchestration send`, `POST /api/factory/send`).
//!
//! A send resolves its target's binding and delivers the one way that binding
//! allows (SPEC §9 "one verb, three deliveries"):
//!
//! | Target | Delivery (`via`) | `state` |
//! |---|---|---|
//! | Terminal bot (a pane) | `pane`: verified paste, or `pane_queue`: its mailbox | `verified` / `queued` |
//! | Hosted bot | `session`: a turn posted to its Gizzi session through allternit-api | `verified` when the session accepted it |
//! | Vendor bot | `vendor_ticket`: a ticket through allternit-api's vendor ticket route | `queued` (the ticket is dispatched on the vendor's lane) |
//! | `channel:<threadId>` | `channel`: relayed through the thread's channel binding | `verified` sent / `best_effort` unconfirmed / `queued` held for approval |
//!
//! Every send — delivered, queued or failed — is recorded twice in the
//! workspace ledger: the text as a `MessageSent` on the bot's mail thread
//! (`messageId`), and the outcome as a `factory.delivery` event carrying the
//! `Delivery`. Nothing reports `verified` that was not verified.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::backend::{self, PaneSend};
use crate::agents::registry::Registry;
use crate::agents::view::{self, Agent};
use crate::core::types::{ActorType, LedgerQuery};
use crate::ledger::{Ledger, LedgerOptions};
use crate::mail::{Mail, MailImportance, MailOptions, TypedMessage};

/// Ledger event type of a recorded delivery.
pub const DELIVERY_EVENT: &str = "factory.delivery";

/// API.md `Delivery`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub to: String,
    /// `session` | `pane` | `pane_queue` | `vendor_ticket` | `channel`.
    pub via: String,
    /// `verified` | `queued` | `best_effort` | `read_only` | `failed`.
    pub state: String,
    pub ticket: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub node_id: Option<String>,
    pub dag_id: Option<String>,
    pub at: String,
    pub detail: Option<String>,
}

/// The `factory.delivery` event payload: the Delivery plus its send key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryRecord {
    #[serde(flatten)]
    delivery: Delivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sender: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendRequest {
    /// Agent id, address (`slug@team`), slug, bot id, or `channel:<threadId>`.
    pub to: String,
    pub text: String,
    /// Put it on the mailbox without trying to paste first (terminal only).
    #[serde(default)]
    pub queue: bool,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub dag_id: Option<String>,
    /// Same key twice returns the first Delivery instead of sending again.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// A send that could not even be attempted. `code` is an API.md error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError {
    pub code: &'static str,
    pub fact: String,
    pub action: String,
}

impl SendError {
    fn new(code: &'static str, fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self { code, fact: fact.into(), action: action.into() }
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.fact)
    }
}

impl std::error::Error for SendError {}

/// How the engine reaches allternit-api for hosted, vendor and channel sends.
#[derive(Debug, Clone, Default)]
pub struct ApiLink {
    /// e.g. `http://127.0.0.1:8013`.
    pub base: Option<String>,
    /// The `Authorization` header value to call it with.
    pub authorization: Option<String>,
}

impl ApiLink {
    /// `$ALLTERNIT_FACTORY_API_URL`, else `http://127.0.0.1:$ALLTERNIT_API_PORT`
    /// (when set); bearer `$ALLTERNIT_FACTORY_API_TOKEN`. allternit-api's proxy
    /// overrides both per request (`x-allternit-api-base`, the caller's own
    /// `Authorization`).
    pub fn from_env() -> Self {
        let base = std::env::var("ALLTERNIT_FACTORY_API_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| {
                std::env::var("ALLTERNIT_API_PORT")
                    .ok()
                    .filter(|p| p.parse::<u16>().is_ok())
                    .map(|p| format!("http://127.0.0.1:{p}"))
            });
        let authorization = std::env::var("ALLTERNIT_FACTORY_API_TOKEN")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|t| format!("Bearer {t}"));
        Self { base, authorization }
    }
}

/// Everything a send needs besides the request.
pub struct SendCtx {
    pub root: PathBuf,
    pub registry: Registry,
    pub api: ApiLink,
    /// Who is sending (`user:<id>`, `agent:<id>`, `engine`).
    pub sender: String,
}

/// What the send will do, before doing it (`--dry-run`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPlan {
    pub to: String,
    pub via: String,
    pub thread_id: String,
    /// Ledger event types the real send appends, in order (besides opening
    /// the mail thread the first time it is used).
    pub records: Vec<String>,
    /// For a paste: what it appends instead if the pane is busy when it
    /// runs and the text goes to the mailbox (`via: pane_queue`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_queued: Option<Vec<String>>,
}

/// The Bus event a mailbox enqueue appends.
const MAILBOX_EVENT: &str = "BusMessageSent";

enum Target {
    Terminal(Agent),
    /// A hosted bot (no vendor binding) that has a thread to post into.
    Hosted(String),
    /// A vendor bot (its execution binding says `vendor`).
    Vendor(String),
    Channel(String),
}

fn ledger(root: &Path) -> Arc<Ledger> {
    Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.to_path_buf()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }))
}

fn mail(root: &Path, sender: &str) -> Mail {
    let (actor_type, actor_id) = match sender.split_once(':') {
        Some(("user", id)) => (ActorType::User, id.to_string()),
        Some(("agent", id)) => (ActorType::Agent, id.to_string()),
        _ => (ActorType::Gate, sender.to_string()),
    };
    Mail::new(MailOptions {
        root_dir: Some(root.to_path_buf()),
        ledger: ledger(root),
        actor_id: Some(actor_id),
        actor_type: Some(actor_type),
        mail_index: None,
    })
}

/// The local agent view (see [`view::snapshot`]).
pub async fn local_agents(root: &Path, registry: &Registry) -> Result<Vec<Agent>> {
    Ok(view::snapshot(root, registry, None).await?.agents)
}

/// The mail thread a send is recorded on: the bot thread it was sent in
/// (`mail:<threadId>`), else the bot's own (`mail:bot-<id>`).
fn thread_for(req: &SendRequest, to: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
            .collect()
    };
    match &req.thread_id {
        Some(t) if ["mail:", "dag:", "wih:"].iter().any(|p| t.starts_with(p)) => t.clone(),
        Some(t) => format!("mail:{}", clean(t)),
        None => format!("mail:bot-{}", clean(to)),
    }
}

async fn resolve(ctx: &SendCtx, req: &SendRequest) -> std::result::Result<Target, SendError> {
    if let Some(thread) = req.to.strip_prefix("channel:") {
        if thread.is_empty() {
            return Err(SendError::new("usage", "channel: needs a thread id", "Send to channel:<threadId>."));
        }
        return Ok(Target::Channel(thread.to_string()));
    }
    let agents = local_agents(&ctx.root, &ctx.registry)
        .await
        .map_err(|e| SendError::new("transport", format!("reading the agent registry: {e:#}"), "Retry; check ~/.allternit/factory."))?;
    if let Some(agent) = view::find(&agents, &req.to) {
        return Ok(Target::Terminal(agent.clone()));
    }
    if ctx.api.base.is_some() {
        return resolve_remote(&ctx.api, &req.to).await;
    }
    Err(SendError::new(
        "not_found",
        format!("no agent {} on this computer, and no allternit-api link to look it up", req.to),
        "List agents with `gizzi agents ps`; for a hosted or vendor bot, send through the app or set ALLTERNIT_FACTORY_API_URL.",
    ))
}

/// Plan a send without doing it.
pub async fn plan(ctx: &SendCtx, req: &SendRequest) -> std::result::Result<SendPlan, SendError> {
    validate(req)?;
    let target = resolve(ctx, req).await?;
    let (to, via) = match &target {
        Target::Terminal(agent) => (
            agent.id.clone(),
            if req.queue || agent.pane.is_none() { "pane_queue" } else { "pane" }.to_string(),
        ),
        Target::Hosted(bot) => (bot.clone(), "session".to_string()),
        Target::Vendor(bot) => (bot.clone(), "vendor_ticket".to_string()),
        Target::Channel(thread) => (format!("channel:{thread}"), "channel".to_string()),
    };
    let base = |extra: Option<&str>| -> Vec<String> {
        let mut r = vec!["MessageSent".to_string()];
        r.extend(extra.map(str::to_string));
        r.push(DELIVERY_EVENT.to_string());
        r
    };
    let (records, if_queued) = match via.as_str() {
        "pane_queue" => (base(Some(MAILBOX_EVENT)), None),
        "pane" => (base(None), Some(base(Some(MAILBOX_EVENT)))),
        _ => (base(None), None),
    };
    Ok(SendPlan { thread_id: thread_for(req, &to), to, via, records, if_queued })
}

fn validate(req: &SendRequest) -> std::result::Result<(), SendError> {
    if req.to.trim().is_empty() {
        return Err(SendError::new("usage", "send needs a target (to)", "Pass an agent id, address or slug."));
    }
    if req.text.trim().is_empty() {
        return Err(SendError::new("usage", "send needs text", "Pass the message text."));
    }
    Ok(())
}

/// Send, record, and return the Delivery. `Err` only when the send could not
/// be attempted at all (usage, unknown target); a failed attempt is an `Ok`
/// Delivery with `state: failed`, recorded like any other.
pub async fn send(ctx: &SendCtx, req: &SendRequest) -> std::result::Result<Delivery, SendError> {
    validate(req)?;
    if let Some(key) = &req.idempotency_key {
        if let Some(existing) = find_by_key(&ctx.root, key).await {
            return Ok(existing);
        }
    }
    let target = resolve(ctx, req).await?;
    let to = match &target {
        Target::Terminal(agent) => agent.id.clone(),
        Target::Hosted(bot) | Target::Vendor(bot) => bot.clone(),
        Target::Channel(thread) => format!("channel:{thread}"),
    };
    let thread_id = thread_for(req, &to);
    let mail = mail(&ctx.root, &ctx.sender);
    let record_err = |e: anyhow::Error| SendError::new("transport", format!("recording the message: {e:#}"), "Check the workspace's .allternit/ledger is writable.");
    let mail_thread = mail.ensure_thread(&thread_id).await.map_err(record_err)?;
    let message_id = mail
        .send_typed_message(
            &mail_thread,
            TypedMessage {
                from_agent: ctx.sender.clone(),
                to_agents: vec![to.clone()],
                subject: Some("send".to_string()),
                body: req.text.clone(),
                importance: MailImportance::Normal,
                ack_required: false,
            },
        )
        .await
        .map_err(record_err)?;

    let (via, state, ticket, detail) = match &target {
        Target::Terminal(agent) => deliver_terminal(ctx, agent, req).await,
        Target::Hosted(bot) => deliver_hosted(&ctx.api, bot, req).await,
        Target::Vendor(bot) => deliver_vendor(&ctx.api, bot, req).await,
        Target::Channel(thread) => deliver_channel(&ctx.api, thread, req, &message_id).await,
    };
    let delivery = Delivery {
        id: format!("dlv_{}", message_id.trim_start_matches("evt_")),
        to,
        via,
        state,
        ticket,
        thread_id: req.thread_id.clone(),
        message_id: Some(message_id),
        node_id: req.node_id.clone(),
        dag_id: req.dag_id.clone(),
        at: chrono::Utc::now().to_rfc3339(),
        detail,
    };
    let record = DeliveryRecord {
        delivery: delivery.clone(),
        idempotency_key: req.idempotency_key.clone(),
        sender: Some(ctx.sender.clone()),
    };
    let payload = serde_json::to_value(&record).unwrap_or(Value::Null);
    mail.log_event(DELIVERY_EVENT, payload).await.map_err(record_err)?;
    Ok(delivery)
}

type Outcome = (String, String, Option<String>, Option<String>);

async fn deliver_terminal(ctx: &SendCtx, agent: &Agent, req: &SendRequest) -> Outcome {
    let session = crate::agents::registry::session_of(&agent.slug);
    let pane = match backend::backend() {
        Ok(p) => p,
        Err(e) => return ("pane".into(), "failed".into(), None, Some(format!("{e:#}"))),
    };
    let (root, text, sender, queue_only) = (ctx.root.clone(), req.text.clone(), ctx.sender.clone(), req.queue || agent.pane.is_none());
    match backend::blocking(move || pane.send(&root, &session, &text, &sender, queue_only)).await {
        Ok(PaneSend::Verified) => ("pane".into(), "verified".into(), None, None),
        Ok(PaneSend::Queued { message_id, depth, reason }) => (
            "pane_queue".into(),
            "queued".into(),
            None,
            Some(format!("{reason}; mailbox message {message_id}, depth {depth}")),
        ),
        Err(e) => ("pane".into(), "failed".into(), None, Some(format!("{e:#}"))),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

async fn api_call(api: &ApiLink, method: reqwest::Method, path: &str, body: Option<Value>) -> std::result::Result<(u16, Value), String> {
    let Some(base) = &api.base else {
        return Err("no allternit-api link (set ALLTERNIT_FACTORY_API_URL)".to_string());
    };
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut rb = client().request(method, &url);
    if let Some(auth) = &api.authorization {
        rb = rb.header("authorization", auth);
    }
    if let Some(body) = body {
        rb = rb.json(&body);
    }
    let resp = rb.send().await.map_err(|e| format!("allternit-api at {base} unreachable: {e}"))?;
    let status = resp.status().as_u16();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    Ok((status, value))
}

fn api_error(v: &Value) -> String {
    v.get("error")
        .and_then(|e| e.as_str().map(str::to_string).or_else(|| e.get("fact").and_then(Value::as_str).map(str::to_string)))
        .or_else(|| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| v.to_string())
}

fn transport_err(fact: String) -> SendError {
    SendError::new("transport", fact, "Check that allternit-api is running and reachable, then send again.")
}

/// A bot that isn't on this computer, looked up through allternit-api: its
/// execution binding says vendor; with no binding it is hosted, if it has a
/// thread. Unknown → `not_found`; allternit-api unusable → `transport`.
/// Nothing is recorded until this succeeds.
async fn resolve_remote(api: &ApiLink, bot: &str) -> std::result::Result<Target, SendError> {
    let enc = urlencoding::encode(bot);
    match api_call(api, reqwest::Method::GET, &format!("/api/v1/gateway/bots/{enc}/execution-binding"), None).await {
        Ok((200, v)) if v["binding"]["type"] == "vendor" => return Ok(Target::Vendor(bot.to_string())),
        Ok((200, _)) | Ok((404, _)) => {}
        Ok((status, v)) => return Err(transport_err(format!("allternit-api could not look up bot {bot} ({status}: {})", api_error(&v)))),
        Err(e) => return Err(transport_err(e)),
    }
    match api_call(api, reqwest::Method::GET, &format!("/api/v1/threads?botId={enc}"), None).await {
        Ok((200, v)) if v["threads"].as_array().is_some_and(|t| !t.is_empty()) => Ok(Target::Hosted(bot.to_string())),
        Ok((200, _)) | Ok((404, _)) => Err(SendError::new(
            "not_found",
            format!("no agent or bot {bot}"),
            "List agents with `gizzi agents ps`; a hosted bot needs a thread before it can be sent to.",
        )),
        Ok((status, v)) => Err(transport_err(format!("allternit-api could not list threads for {bot} ({status}: {})", api_error(&v)))),
        Err(e) => Err(transport_err(e)),
    }
}

async fn deliver_vendor(api: &ApiLink, bot: &str, req: &SendRequest) -> Outcome {
    let enc = urlencoding::encode(bot);
    let Some(thread) = req.thread_id.as_deref() else {
        return (
            "vendor_ticket".into(),
            "failed".into(),
            None,
            Some("a vendor ticket needs the bot thread it belongs to (threadId)".into()),
        );
    };
    let body = json!({ "instructions": req.text, "threadId": thread });
    match api_call(api, reqwest::Method::POST, &format!("/api/v1/vendor-bots/{enc}/tickets"), Some(body)).await {
        Ok((200..=299, v)) => {
            let t = &v["ticket"];
            let ticket = t["n"].as_i64().map(|n| format!("T-{n}"));
            let id = t["id"].as_str().unwrap_or_default();
            ("vendor_ticket".into(), "queued".into(), ticket, Some(format!("ticket {id} created; dispatching on the vendor's lane")))
        }
        Ok((status, v)) => ("vendor_ticket".into(), "failed".into(), None, Some(format!("vendor ticket {status}: {}", api_error(&v)))),
        Err(e) => ("vendor_ticket".into(), "failed".into(), None, Some(e)),
    }
}

/// Hosted: a turn in the bot's Gizzi session (its thread's current session).
async fn deliver_hosted(api: &ApiLink, bot: &str, req: &SendRequest) -> Outcome {
    let enc = urlencoding::encode(bot);
    let thread = match &req.thread_id {
        Some(t) => Some(t.clone()),
        None => match api_call(api, reqwest::Method::GET, &format!("/api/v1/threads?botId={enc}"), None).await {
            Ok((200, v)) => v["threads"].as_array().and_then(|ts| {
                ts.iter()
                    .find(|t| t["kind"] == "standing")
                    .or_else(|| ts.first())
                    .and_then(|t| t["id"].as_str().map(str::to_string))
            }),
            Ok((status, v)) => {
                return ("session".into(), "failed".into(), None, Some(format!("thread lookup {status}: {}", api_error(&v))))
            }
            Err(e) => return ("session".into(), "failed".into(), None, Some(e)),
        },
    };
    let Some(thread) = thread else {
        return ("session".into(), "failed".into(), None, Some(format!("bot {bot} has no thread to post into")));
    };
    let session = match api_call(api, reqwest::Method::GET, &format!("/api/v1/threads/{}", urlencoding::encode(&thread)), None).await {
        Ok((200, v)) => {
            let t = if v.get("thread").is_some() { &v["thread"] } else { &v };
            t["currentSessionId"].as_str().map(str::to_string)
        }
        Ok((status, v)) => {
            return ("session".into(), "failed".into(), None, Some(format!("thread {thread} {status}: {}", api_error(&v))))
        }
        Err(e) => return ("session".into(), "failed".into(), None, Some(e)),
    };
    let Some(session) = session else {
        return ("session".into(), "failed".into(), None, Some(format!("thread {thread} has no live session")));
    };
    let body = json!({ "text": req.text, "role": "user", "metadata": { "source": "factory" } });
    match api_call(api, reqwest::Method::POST, &format!("/api/v1/agent-sessions/{}/messages", urlencoding::encode(&session)), Some(body)).await {
        Ok((200..=299, _)) => ("session".into(), "verified".into(), None, Some(format!("session {session}"))),
        Ok((status, v)) => ("session".into(), "failed".into(), None, Some(format!("session {session} {status}: {}", api_error(&v)))),
        Err(e) => ("session".into(), "failed".into(), None, Some(e)),
    }
}

async fn deliver_channel(api: &ApiLink, thread: &str, req: &SendRequest, message_id: &str) -> Outcome {
    let body = json!({ "text": req.text, "correlationId": message_id });
    let path = format!("/api/v1/gateway/threads/{}/channel-send", urlencoding::encode(thread));
    match api_call(api, reqwest::Method::POST, &path, Some(body)).await {
        Ok((200, v)) => ("channel".into(), "verified".into(), None, v["remoteId"].as_str().map(|r| format!("remote {r}"))),
        Ok((202, _)) => ("channel".into(), "best_effort".into(), None, Some("sent; the channel has not confirmed it".into())),
        Ok((428, v)) => (
            "channel".into(),
            "queued".into(),
            None,
            Some(format!("held for approval {}", v["approvalId"].as_str().unwrap_or("?"))),
        ),
        Ok((403, v)) if v["code"] == "READ_ONLY" => ("channel".into(), "read_only".into(), None, Some(api_error(&v))),
        Ok((status, v)) => ("channel".into(), "failed".into(), None, Some(format!("channel send {status}: {}", api_error(&v)))),
        Err(e) => ("channel".into(), "failed".into(), None, Some(e)),
    }
}

/// Filter for [`deliveries`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeliveryFilter {
    pub agent: Option<String>,
    pub thread: Option<String>,
    pub node: Option<String>,
}

/// Every recorded Delivery (oldest first), filtered. A projection of the
/// ledger's `factory.delivery` events; a later record of the same id wins.
pub async fn deliveries(root: &Path, filter: &DeliveryFilter) -> Result<Vec<Delivery>> {
    let events = ledger(root)
        .query(LedgerQuery { r#type: Some(DELIVERY_EVENT.to_string()), ..Default::default() })
        .await?;
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Delivery> = std::collections::HashMap::new();
    for evt in events {
        let Ok(rec) = serde_json::from_value::<DeliveryRecord>(evt.payload) else { continue };
        let d = rec.delivery;
        if !by_id.contains_key(&d.id) {
            order.push(d.id.clone());
        }
        by_id.insert(d.id.clone(), d);
    }
    Ok(order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .filter(|d| filter.agent.as_deref().map_or(true, |a| d.to == a))
        .filter(|d| filter.thread.as_deref().map_or(true, |t| d.thread_id.as_deref() == Some(t)))
        .filter(|d| filter.node.as_deref().map_or(true, |n| d.node_id.as_deref() == Some(n)))
        .collect())
}

async fn find_by_key(root: &Path, key: &str) -> Option<Delivery> {
    let events = ledger(root)
        .query(LedgerQuery { r#type: Some(DELIVERY_EVENT.to_string()), ..Default::default() })
        .await
        .ok()?;
    events.into_iter().find_map(|evt| {
        let rec = serde_json::from_value::<DeliveryRecord>(evt.payload).ok()?;
        (rec.idempotency_key.as_deref() == Some(key)).then_some(rec.delivery)
    })
}
