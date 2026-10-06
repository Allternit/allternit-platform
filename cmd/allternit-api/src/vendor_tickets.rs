//! Vendor task tickets, lane ranking and the nudge (migration V222).
//!
//! A directing bot hands a vendor bot (Claude, ChatGPT, Grok ...) a **ticket**: `T-<n>`,
//! instructions, the tools it may use, a deadline. The structured part travels through the
//! vendor-bot MCP connector (`get_ticket`, `post_result`, `list_open_tickets` in
//! [`crate::mcp_vendor_bots`]); the vendor's own chat only gets a one-line **nudge**.
//!
//! **Lanes**, best first, per vendor account. [`pick_lane`] takes the highest that is available:
//! 1. `local_app`: the vendor's app on this computer (`claude -p`, `codex exec`, `gemini -p`,
//!    `hermes chat -q`), on the person's own subscription, with Allternit's local MCP server
//!    registered in its config ([`crate::vendor_local_connector`]).
//! 2. `website_connector`: the vendor's website plus a remote connector (Claude.ai, ChatGPT dev mode).
//! 3. `website_only`: the website alone (Grok and others), today's `ui_bridge`.
//! 4. `api_key`: an API key with a remote MCP connector.
//!
//! **Nudge.** Lanes 1, 2 and 4 send "Run Allternit ticket T-n" plus a connector hint. Lane 3
//! has no connector, so it gets the full task and its reply is the result.
//!
//! **Completion** is `post_result` through the connector: the result lands in the thread as a
//! `vendor.ticket.result` card. If none arrives by the deadline the vendor's reply is read
//! instead (`resultVia: "reply"`); with no reply either the ticket expires.
//!
//! **Node deliveries** (Factory, SPEC §9 "Vendor tickets"). A ticket can be linked to a Factory
//! node (`dag_id`, `node_id`, `wih_id`, `workspace_root`; migration V239): it is then how that
//! node reaches the vendor bot. When it completes, its result is recorded as the node's output
//! and the WIH is closed through the Gate ([`crate::factory_bots::close_linked_node`]). A refused
//! close keeps the result and shows up as `nodeClose.state = "failed"` with the reason.
//!
//! Inert unless used: nothing here runs until a ticket is created.

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

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::vendor_local_connector as local;
use crate::AppState;

pub const LANE_LOCAL: &str = "local_app";
pub const LANE_WEBSITE_CONNECTOR: &str = "website_connector";
pub const LANE_WEBSITE_ONLY: &str = "website_only";
pub const LANE_API_KEY: &str = "api_key";

const DEFAULT_DEADLINE_SECS: i64 = 600;
const MAX_INSTRUCTIONS: usize = 8000;
const MAX_SUMMARY: usize = 4000;
const MAX_DATA_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENTS: usize = 10;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ─── Lane ranking ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaneFacts {
    /// A connected, runnable local app for this vendor (`claude_code`, `codex`, ...).
    pub local_app: Option<String>,
    /// A signed-in browser account that is healthy.
    pub website_connected: bool,
    /// The vendor connected the remote Allternit connector (a live OAuth client).
    pub connector_connected: bool,
    /// An API-key account with a key, for a vendor that takes remote MCP.
    pub api_key_ready: bool,
}

const ALL_LANES: [&str; 4] = [LANE_LOCAL, LANE_WEBSITE_CONNECTOR, LANE_WEBSITE_ONLY, LANE_API_KEY];

fn lane_available(f: &LaneFacts, lane: &str) -> bool {
    match lane {
        LANE_LOCAL => f.local_app.is_some(),
        LANE_WEBSITE_CONNECTOR => f.website_connected && f.connector_connected,
        LANE_WEBSITE_ONLY => f.website_connected,
        _ => f.api_key_ready,
    }
}

pub fn lane_label(lane: &str) -> &'static str {
    match lane {
        LANE_LOCAL => "Local app",
        LANE_WEBSITE_CONNECTOR => "Website and connector",
        LANE_WEBSITE_ONLY => "Website only",
        _ => "API key",
    }
}

/// The highest lane that is available and healthy.
pub fn pick_lane(f: &LaneFacts) -> Option<&'static str> {
    ALL_LANES.into_iter().find(|l| lane_available(f, l))
}

pub fn rank_lanes(f: &LaneFacts) -> Vec<Value> {
    ALL_LANES
        .iter()
        .enumerate()
        .map(|(i, l)| json!({ "lane": l, "rank": i + 1, "label": lane_label(l), "available": lane_available(f, l) }))
        .collect()
}

pub fn lanes_view(f: &LaneFacts) -> Value {
    json!({ "selected": pick_lane(f), "lanes": rank_lanes(f), "localApp": f.local_app })
}

/// The lane facts for one vendor account (and, when given, one vendor bot's connector clients).
pub fn lane_facts(db: &DbHandle, owner: &str, account_id: &str, vendor_bot_id: Option<&str>) -> Result<LaneFacts, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let acct: Option<(String, String, String, bool, String)> = conn
        .query_row(
            "SELECT vendor, auth_type, state, secret_ref IS NOT NULL, host_kind FROM provider_account_bindings WHERE id = ?1 AND owner = ?2",
            params![account_id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((vendor, auth_type, state, has_secret, host_kind)) = acct else { return Err("account not found".into()) };
    let healthy = state == "CONNECTED";
    // Local apps run on the host computer, so an account hosted on a cloud computer has none.
    let connected = local::connected_apps(db, owner);
    let local_app = if host_kind == "this_device" {
        local::vendor_apps(&vendor).iter().find(|a| local::runnable(a) && connected.iter().any(|c| c == *a)).map(|a| a.to_string())
    } else {
        None
    };
    let connector_connected = conn
        .query_row(
            "SELECT 1 FROM vendor_connector_clients c WHERE c.owner = ?1 AND c.revoked_at IS NULL AND c.client NOT LIKE 'local-key:%'
               AND (?3 IS NOT NULL AND c.vendor_bot_id = ?3 OR ?3 IS NULL AND c.vendor_bot_id IN
                    (SELECT bot_id FROM bot_execution_bindings WHERE owner = ?1 AND account_binding_id = ?2)) LIMIT 1",
            params![owner, account_id, vendor_bot_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    Ok(LaneFacts {
        local_app,
        website_connected: healthy && auth_type == "browser_session",
        connector_connected: connector_connected && matches!(vendor.as_str(), "anthropic" | "openai"),
        api_key_ready: healthy && auth_type == "api_key" && has_secret && matches!(vendor.as_str(), "xai" | "anthropic" | "openai"),
    })
}

// ─── Nudge ─────────────────────────────────────────────────────────────────────

/// What is sent to the vendor for `ticket` on `lane`.
pub fn nudge(lane: &str, ticket: &Value, connector_url: &str) -> String {
    let id = ticket["id"].as_str().unwrap_or_default();
    match lane {
        LANE_WEBSITE_ONLY => {
            let tools = ticket["allowedTools"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let mut out = format!("Allternit ticket {id}\n\n{}", ticket["instructions"].as_str().unwrap_or_default());
            if !tools.is_empty() {
                out.push_str(&format!("\n\nTools you may use: {tools}."));
            }
            out.push_str("\n\nWhen you are done, reply with the result.");
            out
        }
        _ => format!("Run Allternit ticket {id}. Use the Allternit connector ({connector_url}): get_ticket {{\"id\":\"{id}\"}}, then post_result when done."),
    }
}

// ─── Tickets ───────────────────────────────────────────────────────────────────

pub(crate) const COLS: &str = "id, n, directing_bot_id, vendor_bot_id, thread_id, instructions, allowed_tools, status, lane, result_json, result_via, error, deadline_at, created_at, updated_at, completed_at, \
    dag_id, node_id, wih_id, workspace_root, node_close_state, node_close_error, node_close_json, node_close_at";

pub(crate) fn ticket_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let lane = r.get::<_, Option<String>>(8)?;
    let dag_id = r.get::<_, Option<String>>(16)?;
    let close_state = r.get::<_, Option<String>>(20)?;
    let close = parse(r.get(22)?).unwrap_or(json!({}));
    let node_close = close_state.map(|state| {
        json!({ "state": state, "error": r.get::<_, Option<String>>(21).ok().flatten(), "at": r.get::<_, Option<String>>(23).ok().flatten(),
                "receiptId": close["receiptId"], "finalStatus": close["finalStatus"], "nodeStatus": close["nodeStatus"] })
    });
    Ok(json!({
        "id": r.get::<_, String>(0)?, "n": r.get::<_, i64>(1)?, "directingBotId": r.get::<_, Option<String>>(2)?,
        "vendorBotId": r.get::<_, String>(3)?, "threadId": r.get::<_, String>(4)?, "instructions": r.get::<_, String>(5)?,
        "allowedTools": parse(r.get(6)?).unwrap_or(json!([])), "status": r.get::<_, String>(7)?, "lane": lane,
        "guarantee": lane.as_deref().map(lane_guarantee),
        "result": parse(r.get(9)?), "resultVia": r.get::<_, Option<String>>(10)?, "error": r.get::<_, Option<String>>(11)?,
        "deadlineAt": r.get::<_, String>(12)?, "createdAt": r.get::<_, String>(13)?, "updatedAt": r.get::<_, String>(14)?, "completedAt": r.get::<_, Option<String>>(15)?,
        "dagId": dag_id, "nodeId": r.get::<_, Option<String>>(17)?, "wihId": r.get::<_, Option<String>>(18)?,
        "workspaceRoot": r.get::<_, Option<String>>(19)?, "nodeClose": node_close,
    }))
}

/// What a lane can promise about delivery (SPEC §9: a `best_effort` delivery is never shown as read).
/// A local run returns its own output and an API key call is direct (`exact`); the website lanes
/// paste a nudge into a vendor's web chat (`best_effort`).
pub fn lane_guarantee(lane: &str) -> &'static str {
    match lane {
        LANE_LOCAL | LANE_API_KEY => "exact",
        _ => "best_effort",
    }
}

pub fn get_ticket(db: &DbHandle, owner: &str, id: &str) -> Result<Option<Value>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    conn.query_row(&format!("SELECT {COLS} FROM vendor_tickets WHERE owner = ?1 AND id = ?2"), params![owner, id], ticket_json).optional().map_err(|e| e.to_string())
}

pub fn list_tickets(db: &DbHandle, owner: &str, vendor_bot_id: &str, open_only: bool) -> Result<Vec<Value>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let filter = if open_only { "AND status IN ('open','sent')" } else { "" };
    let mut q = conn
        .prepare(&format!("SELECT {COLS} FROM vendor_tickets WHERE owner = ?1 AND vendor_bot_id = ?2 {filter} ORDER BY n DESC LIMIT 100"))
        .map_err(|e| e.to_string())?;
    let rows = q.query_map(params![owner, vendor_bot_id], ticket_json).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok(rows)
}

#[derive(Default)]
pub struct NewTicket<'a> {
    pub owner: &'a str,
    pub vendor_bot_id: &'a str,
    pub directing_bot_id: Option<&'a str>,
    pub thread_id: &'a str,
    pub instructions: &'a str,
    pub allowed_tools: Vec<String>,
    pub deadline_secs: Option<i64>,
    /// The Factory node this ticket delivers (V239). Unique per (owner, dag, node, WIH).
    pub node: Option<NodeLink<'a>>,
}

/// A ticket's Factory node: the WIH it closes and the workspace whose Gate closes it.
#[derive(Clone, Copy, Debug)]
pub struct NodeLink<'a> {
    pub dag_id: &'a str,
    pub node_id: &'a str,
    pub wih_id: &'a str,
    pub workspace_root: &'a str,
}

/// The ticket already delivering (dag, node, WIH) for `owner`, if any.
pub fn ticket_for_node(db: &DbHandle, owner: &str, dag_id: &str, node_id: &str, wih_id: &str) -> Result<Option<Value>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    conn.query_row(
        &format!("SELECT {COLS} FROM vendor_tickets WHERE owner = ?1 AND dag_id = ?2 AND node_id = ?3 AND wih_id = ?4"),
        params![owner, dag_id, node_id, wih_id],
        ticket_json,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// The owner's tickets for a DAG (and node), newest first.
pub fn tickets_for_node(db: &DbHandle, owner: &str, dag_id: Option<&str>, node_id: Option<&str>) -> Result<Vec<Value>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut q = conn
        .prepare(&format!(
            "SELECT {COLS} FROM vendor_tickets WHERE owner = ?1 AND dag_id IS NOT NULL AND (?2 IS NULL OR dag_id = ?2) AND (?3 IS NULL OR node_id = ?3) ORDER BY n DESC LIMIT 200"
        ))
        .map_err(|e| e.to_string())?;
    let rows = q.query_map(params![owner, dag_id, node_id], ticket_json).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok(rows)
}

/// The thread's bot, when the thread is the owner's.
fn thread_bot(db: &DbHandle, owner: &str, thread_id: &str) -> Option<String> {
    db.connect().ok()?.query_row("SELECT bot_id FROM bot_threads WHERE id = ?1 AND user_id = ?2", params![thread_id, owner], |r| r.get(0)).optional().ok().flatten()
}

pub fn create_ticket(db: &DbHandle, t: NewTicket) -> Result<Value, String> {
    let instructions = t.instructions.trim();
    if instructions.is_empty() {
        return Err("Say what the ticket is for.".into());
    }
    if instructions.chars().count() > MAX_INSTRUCTIONS {
        return Err("The instructions are too long. Shorten them.".into());
    }
    if thread_bot(db, t.owner, t.thread_id).is_none() {
        return Err("That thread isn't one of yours.".into());
    }
    let secs = t.deadline_secs.unwrap_or(DEFAULT_DEADLINE_SECS).clamp(30, 86_400);
    let created = chrono::Utc::now();
    let deadline = (created + chrono::Duration::seconds(secs)).to_rfc3339();
    if let Some(l) = &t.node {
        if [l.dag_id, l.node_id, l.wih_id, l.workspace_root].iter().any(|v| v.trim().is_empty()) {
            return Err("A node ticket needs dagId, nodeId, wihId and workspaceRoot.".into());
        }
        if let Some(existing) = ticket_for_node(db, t.owner, l.dag_id, l.node_id, l.wih_id)? {
            return Ok(existing);
        }
    }
    let conn = db.connect().map_err(|e| e.to_string())?;
    let n: i64 = conn.query_row("SELECT COALESCE(MAX(n), 0) + 1 FROM vendor_tickets WHERE owner = ?1", params![t.owner], |r| r.get(0)).map_err(|e| e.to_string())?;
    let id = format!("T-{n}");
    let tools = serde_json::to_string(&t.allowed_tools).unwrap();
    let link = t.node;
    let inserted = conn.execute(
        "INSERT INTO vendor_tickets (owner, id, n, directing_bot_id, vendor_bot_id, thread_id, instructions, allowed_tools, status, deadline_at, created_at, updated_at,
                                     dag_id, node_id, wih_id, workspace_root)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'open',?9,?10,?10,?11,?12,?13,?14)",
        params![t.owner, id, n, t.directing_bot_id, t.vendor_bot_id, t.thread_id, instructions, tools, deadline, created.to_rfc3339(),
                link.map(|l| l.dag_id), link.map(|l| l.node_id), link.map(|l| l.wih_id), link.map(|l| l.workspace_root)],
    );
    if let Err(e) = inserted {
        // A concurrent create for the same node won the unique index: that ticket is the answer.
        if let Some(l) = link {
            if let Some(existing) = ticket_for_node(db, t.owner, l.dag_id, l.node_id, l.wih_id)? {
                return Ok(existing);
            }
        }
        return Err(e.to_string());
    }
    record_participation(db, t.owner, t.vendor_bot_id, t.thread_id, "ticket");
    // Ledger → Desktop/cloud event backbone (`vendor.ticket.created`), on the owner's thread bot.
    if let Some(owner_bot) = thread_bot(db, t.owner, t.thread_id) {
        crate::gateway_runner::led(
            db,
            &owner_bot,
            t.thread_id,
            None,
            "vendor.ticket.created",
            ("user", t.owner),
            json!({ "ticketId": &id, "vendorBotId": t.vendor_bot_id, "deadlineAt": &deadline, "title": instructions.chars().take(120).collect::<String>() }),
            Some(format!("vendor-ticket-created:{}:{id}", t.owner)),
        );
    }
    get_ticket(db, t.owner, &id)?.ok_or_else(|| "ticket missing".into())
}

fn set_status(db: &DbHandle, owner: &str, id: &str, from: &[&str], to: &str, lane: Option<&str>, error: Option<&str>) -> bool {
    let Ok(conn) = db.connect() else { return false };
    let list = from.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",");
    conn.execute(
        &format!("UPDATE vendor_tickets SET status = ?3, lane = COALESCE(?4, lane), error = ?5, updated_at = ?6 WHERE owner = ?1 AND id = ?2 AND status IN ({list})"),
        params![owner, id, to, lane, error, now()],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Finish a ticket (once) and put the result in its thread as a card. A ticket linked to a
/// Factory node also closes that node's WIH through the Gate, in the background.
pub fn complete(db: &DbHandle, owner: &str, id: &str, result: Value, via: &str) -> Result<Value, String> {
    let done = complete_ticket(db, owner, id, result, via)?;
    if done["nodeClose"]["state"] == "pending" {
        crate::factory_bots::spawn_node_close(db.clone(), owner.to_string(), id.to_string());
    }
    Ok(done)
}

/// [`complete`] without starting the node close: a linked ticket is left with
/// `nodeClose.state = "pending"` for the caller to close (and await).
pub fn complete_ticket(db: &DbHandle, owner: &str, id: &str, result: Value, via: &str) -> Result<Value, String> {
    let t = get_ticket(db, owner, id)?.ok_or("There's no such ticket.")?;
    let ts = now();
    let n = db
        .connect()
        .map_err(|e| e.to_string())?
        .execute(
            "UPDATE vendor_tickets SET status = 'done', result_json = ?3, result_via = ?4, pending_reply = NULL, error = NULL, updated_at = ?5, completed_at = ?5,
                    node_close_state = CASE WHEN dag_id IS NOT NULL THEN 'pending' ELSE node_close_state END
             WHERE owner = ?1 AND id = ?2 AND status IN ('open','sent')",
            params![owner, id, result.to_string(), via, ts],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("{id} is already {}.", t["status"].as_str().unwrap_or("closed")));
    }
    let (bot, thread) = (t["vendorBotId"].as_str().unwrap_or_default(), t["threadId"].as_str().unwrap_or_default());
    record_participation(db, owner, bot, thread, "ticket");
    let owner_bot = thread_bot(db, owner, thread).unwrap_or_else(|| bot.to_string());
    crate::gateway_runner::led(
        db,
        &owner_bot,
        thread,
        None,
        "vendor.ticket.result",
        ("vendor", bot),
        json!({ "kind": "vendor_ticket_result", "ticketId": id, "vendorBotId": bot, "via": via, "lane": t["lane"], "summary": result["summary"], "data": result["data"], "attachments": result["attachments"], "text": result["summary"] }),
        Some(format!("vendor-ticket-result:{owner}:{id}")),
    );
    get_ticket(db, owner, id)?.ok_or_else(|| "ticket missing".into())
}

/// Tickets past their deadline: the vendor's reply is the result when we have one, else expired.
/// Returns the ids that changed.
pub fn settle_deadlines(db: &DbHandle, owner: Option<&str>, now_rfc: &str) -> Vec<(String, String)> {
    let Ok(conn) = db.connect() else { return vec![] };
    let due: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT owner, id, pending_reply FROM vendor_tickets WHERE status IN ('open','sent') AND deadline_at <= ?1 AND (?2 IS NULL OR owner = ?2)")
        .and_then(|mut q| q.query_map(params![now_rfc, owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|it| it.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut changed = vec![];
    for (o, id, reply) in due {
        let done = match reply.filter(|r| !r.trim().is_empty()) {
            Some(r) => complete(db, &o, &id, json!({ "summary": r.chars().take(MAX_SUMMARY).collect::<String>() }), "reply").is_ok(),
            None => set_status(db, &o, &id, &["open", "sent"], "expired", None, Some("No result arrived before the deadline.")),
        };
        if done {
            changed.push((o, id));
        }
    }
    changed
}

pub fn cancel_ticket(db: &DbHandle, owner: &str, id: &str) -> bool {
    set_status(db, owner, id, &["open", "sent"], "cancelled", None, None)
}

// ─── Thread attribution ────────────────────────────────────────────────────────

/// Record that a vendor bot took part in a thread (a ticket, or a connector tool call on it).
pub fn record_participation(db: &DbHandle, owner: &str, vendor_bot_id: &str, thread_id: &str, source: &str) {
    if thread_bot(db, owner, thread_id).is_none() {
        return;
    }
    if let Ok(c) = db.connect() {
        let ts = now();
        let _ = c.execute(
            "INSERT INTO vendor_bot_threads (owner, vendor_bot_id, thread_id, source, first_at, last_at) VALUES (?1,?2,?3,?4,?5,?5)
             ON CONFLICT(vendor_bot_id, thread_id) DO UPDATE SET last_at = excluded.last_at WHERE owner = excluded.owner",
            params![owner, vendor_bot_id, thread_id, source, ts],
        );
    }
}

pub fn participated_threads(db: &DbHandle, owner: &str, vendor_bot_id: &str) -> Result<Vec<Value>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut q = conn
        .prepare(
            "SELECT p.thread_id, t.title, t.status, t.last_activity_at, t.bot_id, p.source, p.first_at, p.last_at
             FROM vendor_bot_threads p JOIN bot_threads t ON t.id = p.thread_id AND t.user_id = p.owner
             WHERE p.owner = ?1 AND p.vendor_bot_id = ?2 ORDER BY p.last_at DESC LIMIT 100",
        )
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map(params![owner, vendor_bot_id], |r| {
            Ok(json!({ "threadId": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?, "status": r.get::<_, String>(2)?, "lastActivityAt": r.get::<_, String>(3)?,
                       "botId": r.get::<_, String>(4)?, "source": r.get::<_, String>(5)?, "firstAt": r.get::<_, String>(6)?, "lastAt": r.get::<_, String>(7)? }))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

// ─── Connector tools ───────────────────────────────────────────────────────────

fn arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

pub fn tool_get_ticket(db: &DbHandle, owner: &str, vendor_bot_id: &str, args: &Value) -> Result<Value, String> {
    let id = arg(args, "id").ok_or("id is required.")?;
    settle_deadlines(db, Some(owner), &now());
    // A ticket for another vendor bot reads like no ticket.
    match get_ticket(db, owner, id)? {
        Some(t) if t["vendorBotId"] == vendor_bot_id => Ok(t),
        _ => Err(format!("There's no ticket {id} for you.")),
    }
}

pub fn tool_list_open_tickets(db: &DbHandle, owner: &str, vendor_bot_id: &str) -> Result<Value, String> {
    settle_deadlines(db, Some(owner), &now());
    Ok(json!({ "tickets": list_tickets(db, owner, vendor_bot_id, true)? }))
}

pub fn tool_post_result(db: &DbHandle, owner: &str, vendor_bot_id: &str, args: &Value) -> Result<Value, String> {
    let id = arg(args, "id").ok_or("id is required.")?;
    let summary = arg(args, "summary").ok_or("summary is required.")?;
    if summary.chars().count() > MAX_SUMMARY {
        return Err("The summary is too long. Shorten it.".into());
    }
    let data = args.get("data").cloned().filter(|d| !d.is_null());
    if data.as_ref().is_some_and(|d| d.to_string().len() > MAX_DATA_BYTES) {
        return Err("The data is too large. Send a smaller result.".into());
    }
    let attachments = args.get("attachments").cloned().filter(|a| !a.is_null()).unwrap_or(json!([]));
    let list = attachments.as_array().ok_or("attachments must be a list.")?;
    if list.len() > MAX_ATTACHMENTS {
        return Err(format!("At most {MAX_ATTACHMENTS} attachments."));
    }
    if list.iter().any(|a| !a.is_object() || a["url"].as_str().is_some_and(|u| !u.starts_with("https://"))) {
        return Err("Each attachment is an object, and its url must be https.".into());
    }
    let t = tool_get_ticket(db, owner, vendor_bot_id, &json!({ "id": id }))?;
    // A linked node is closed by the caller (`mcp_vendor_bots::call_tool` awaits it), so the
    // vendor's post_result answer can say whether the node closed.
    let done = complete_ticket(db, owner, id, json!({ "summary": summary, "data": data, "attachments": attachments }), "connector")?;
    let _ = t;
    Ok(json!({ "ok": true, "ticket": done }))
}

// ─── Dispatch ──────────────────────────────────────────────────────────────────

/// How a nudge reaches a vendor. Production = [`LiveSender`]; tests fake it.
#[async_trait]
pub trait Sender: Send + Sync {
    /// Run `prompt` in the vendor's app on this computer; its output is the reply.
    async fn local(&self, app: &str, server_name: &str, prompt: &str, timeout_secs: u64) -> Result<String, String>;
    /// Send `text` into the vendor bot's thread through the Agent Gateway; the vendor's reply, if any.
    async fn vendor(&self, owner: &str, thread_id: &str, ticket_id: &str, text: &str) -> Result<Option<String>, String>;
}

pub struct LiveSender {
    pub state: Arc<AppState>,
    pub runner: Arc<dyn local::CmdRunner>,
}

#[async_trait]
impl Sender for LiveSender {
    async fn local(&self, app: &str, server_name: &str, prompt: &str, timeout_secs: u64) -> Result<String, String> {
        let (program, args) = local::headless_command(app, server_name, prompt).ok_or("That app can't run a ticket.")?;
        let out = self.runner.run(&program, &args, timeout_secs).await?;
        if out.ok { Ok(out.stdout) } else { Err(format!("{} stopped with an error: {}", local::app_label(app), out.stderr.trim().chars().take(200).collect::<String>())) }
    }

    async fn vendor(&self, owner: &str, thread_id: &str, ticket_id: &str, text: &str) -> Result<Option<String>, String> {
        let session: Option<String> = self
            .state
            .db
            .connect()
            .map_err(|e| e.to_string())?
            .query_row("SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1", params![thread_id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let session = session.ok_or("That thread has no session yet.")?;
        let tx = crate::gateway_runner::transport(&self.state);
        let rt = crate::coordinator_routes::GizziCoordinator { state: self.state.clone() };
        let opts = crate::gateway_runner::TurnOpts { correlation_id: Some(format!("ticket-{owner}-{ticket_id}")), ..Default::default() };
        match crate::gateway_runner::run_turn(&self.state.db, tx.as_ref(), &rt, &session, text, opts).await {
            Ok(Some(report)) => Ok(report.reply),
            Ok(None) => Err("That thread isn't a vendor thread.".into()),
            Err(e) => Err(e.message),
        }
    }
}

/// Send ticket `id` on its best lane. Returns the ticket as it stands afterwards.
pub async fn dispatch(db: &DbHandle, sender: &dyn Sender, owner: &str, id: &str) -> Result<Value, String> {
    let t = get_ticket(db, owner, id)?.ok_or("There's no such ticket.")?;
    if t["status"] != "open" {
        return Ok(t);
    }
    let vendor_bot = t["vendorBotId"].as_str().unwrap_or_default().to_string();
    let account: Option<String> = db
        .connect()
        .map_err(|e| e.to_string())?
        .query_row("SELECT account_binding_id FROM bot_execution_bindings WHERE owner = ?1 AND bot_id = ?2", params![owner, vendor_bot], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    let facts = match &account {
        Some(a) => lane_facts(db, owner, a, Some(&vendor_bot))?,
        None => LaneFacts::default(),
    };
    let Some(lane) = pick_lane(&facts) else {
        set_status(db, owner, id, &["open"], "failed", None, Some("No lane is available for this vendor account."));
        return get_ticket(db, owner, id)?.ok_or_else(|| "ticket missing".into());
    };
    let mut text = nudge(lane, &t, &local::local_mcp_url(&vendor_bot));
    // The owner's twin: a lane with no connector gets it inline (shared facts only), the others a pointer.
    if let Some(block) = db.connect().ok().and_then(|c| crate::twin_persona::context_block(&c, owner, crate::twin_persona::Audience::Vendor)) {
        if lane == LANE_WEBSITE_ONLY {
            text.push_str(&format!("\n\n{block}"));
        } else {
            text.push_str(" Call twin_context first for how to speak for the owner.");
        }
    }
    if !set_status(db, owner, id, &["open"], "sent", Some(lane), None) {
        return get_ticket(db, owner, id)?.ok_or_else(|| "ticket missing".into());
    }
    let thread = t["threadId"].as_str().unwrap_or_default();
    let sent: Result<Option<String>, String> = if lane == LANE_LOCAL {
        let app = facts.local_app.clone().unwrap_or_default();
        let server = local::connected_server(db, owner, &app, &vendor_bot).unwrap_or_else(|| local::server_name(&vendor_bot));
        let secs = chrono::DateTime::parse_from_rfc3339(t["deadlineAt"].as_str().unwrap_or_default()).map(|d| (d.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds().max(30) as u64).unwrap_or(600);
        sender.local(&app, &server, &text, secs).await.map(Some)
    } else {
        sender.vendor(owner, thread, id, &text).await
    };
    match sent {
        Err(e) => {
            set_status(db, owner, id, &["sent"], "failed", None, Some(&e));
        }
        Ok(reply) => {
            let reply = reply.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
            // The connector may have answered during the turn; if not, the reply is kept for the deadline.
            let cur = get_ticket(db, owner, id)?.unwrap_or(Value::Null);
            if cur["status"] == "sent" {
                if let Some(r) = &reply {
                    if let Ok(c) = db.connect() {
                        let _ = c.execute("UPDATE vendor_tickets SET pending_reply = ?3 WHERE owner = ?1 AND id = ?2", params![owner, id, r.chars().take(MAX_SUMMARY * 4).collect::<String>()]);
                    }
                    // Lane 3 sent the whole task: its reply is the result. A local run has ended, so its output is final too.
                    if lane == LANE_WEBSITE_ONLY || lane == LANE_LOCAL {
                        let _ = complete(db, owner, id, json!({ "summary": r.chars().take(MAX_SUMMARY).collect::<String>() }), "reply");
                    }
                }
            }
        }
    }
    get_ticket(db, owner, id)?.ok_or_else(|| "ticket missing".into())
}

// ─── HTTP ──────────────────────────────────────────────────────────────────────

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/vendor-bots", get(list_bots_h))
        .route("/v1/vendor-bots/:id/threads", get(threads_h))
        .route("/v1/vendor-bots/:id/tickets", post(create_h).get(tickets_h))
        .route("/v1/vendor-bots/:id/tickets/:ticket_id", get(ticket_h))
        .route("/v1/vendor-bots/:id/tickets/:ticket_id/cancel", post(cancel_h))
        .route("/v1/provider-accounts/:id/lanes", get(lanes_h))
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn not_found() -> Response {
    err(StatusCode::NOT_FOUND, "not_found")
}

fn owned(state: &AppState, user: &AuthUser, id: &str) -> Option<crate::mcp_vendor_bots::Session> {
    crate::mcp_vendor_bots::load_session(&state.db, &user.user_id, id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    instructions: String,
    thread_id: String,
    #[serde(default)]
    allowed_tools: Vec<String>,
    directing_bot_id: Option<String>,
    deadline_seconds: Option<i64>,
    /// Create the ticket without sending it (default: send).
    #[serde(default)]
    hold: bool,
}

async fn create_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<CreateBody>) -> Response {
    let Some(session) = owned(&state, &user, &id) else { return not_found() };
    let directing = b.directing_bot_id.clone().or(session.directing_bot_id.clone());
    let ticket = create_ticket(
        &state.db,
        NewTicket { owner: &user.user_id, vendor_bot_id: &id, directing_bot_id: directing.as_deref(), thread_id: &b.thread_id, instructions: &b.instructions, allowed_tools: b.allowed_tools, deadline_secs: b.deadline_seconds, node: None },
    );
    let ticket = match ticket {
        Ok(t) => t,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    if !b.hold {
        spawn_dispatch(&state, &user.user_id, &ticket);
    }
    (StatusCode::CREATED, Json(json!({ "ticket": ticket }))).into_response()
}

/// Send `ticket` on its best lane in the background, then settle it at its deadline
/// (the vendor's reply is read if the connector never answered).
pub fn spawn_dispatch(state: &Arc<AppState>, owner: &str, ticket: &Value) {
    let (st, owner, tid) = (state.clone(), owner.to_string(), ticket["id"].as_str().unwrap_or_default().to_string());
    let deadline = ticket["deadlineAt"].as_str().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()).map(|d| d.with_timezone(&chrono::Utc));
    tokio::spawn(async move {
        let sender = LiveSender { state: st.clone(), runner: Arc::new(local::SystemRunner) };
        let _ = dispatch(&st.db, &sender, &owner, &tid).await;
        if let Some(d) = deadline {
            let wait = (d - chrono::Utc::now()).num_seconds().max(0) as u64;
            tokio::time::sleep(std::time::Duration::from_secs(wait + 1)).await;
            settle_deadlines(&st.db, Some(&owner), &now());
        }
    });
}

async fn tickets_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    settle_deadlines(&state.db, Some(&user.user_id), &now());
    match list_tickets(&state.db, &user.user_id, &id, false) {
        Ok(t) => Json(json!({ "tickets": t })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn ticket_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((id, ticket_id)): Path<(String, String)>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    settle_deadlines(&state.db, Some(&user.user_id), &now());
    match get_ticket(&state.db, &user.user_id, &ticket_id) {
        Ok(Some(t)) if t["vendorBotId"] == id.as_str() => Json(json!({ "ticket": t })).into_response(),
        _ => not_found(),
    }
}

async fn cancel_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((id, ticket_id)): Path<(String, String)>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    match get_ticket(&state.db, &user.user_id, &ticket_id) {
        Ok(Some(t)) if t["vendorBotId"] == id.as_str() => {
            cancel_ticket(&state.db, &user.user_id, &ticket_id);
            Json(json!({ "ticket": get_ticket(&state.db, &user.user_id, &ticket_id).ok().flatten() })).into_response()
        }
        _ => not_found(),
    }
}

async fn threads_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    if owned(&state, &user, &id).is_none() {
        return not_found();
    }
    match participated_threads(&state.db, &user.user_id, &id) {
        Ok(t) => Json(json!({ "threads": t })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn lanes_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    match lane_facts(&state.db, &user.user_id, &id, None) {
        Ok(f) => Json(lanes_view(&f)).into_response(),
        Err(_) => not_found(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    directing_bot_id: Option<String>,
}

/// The vendor bots deployed for a directing bot, and the vendor agents found on the owner's
/// accounts that aren't deployed yet.
pub fn deployed_bots(db: &DbHandle, owner: &str, directing_bot_id: Option<&str>) -> Result<Vec<Value>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut q = conn
        .prepare(
            "SELECT b.bot_id, a.name, b.vendor, b.account_binding_id, b.external_agent_id, b.external_agent_name, b.state,
                    COALESCE(b.directing_bot_id, c.directing_bot_id)
             FROM bot_execution_bindings b JOIN agents a ON a.id = b.bot_id AND a.user_id = b.owner
             LEFT JOIN vendor_bot_connectors c ON c.vendor_bot_id = b.bot_id AND c.owner = b.owner
             WHERE b.owner = ?1 AND b.type = 'vendor' AND (?2 IS NULL OR COALESCE(b.directing_bot_id, c.directing_bot_id) = ?2)
             ORDER BY b.created_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map(params![owner, directing_bot_id], |r| {
            Ok(json!({ "vendorBotId": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "vendor": r.get::<_, Option<String>>(2)?, "accountBindingId": r.get::<_, Option<String>>(3)?,
                       "externalAgentId": r.get::<_, Option<String>>(4)?, "externalAgentName": r.get::<_, Option<String>>(5)?, "state": r.get::<_, String>(6)?, "directingBotId": r.get::<_, Option<String>>(7)? }))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

async fn list_bots_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<ListQuery>) -> Response {
    let deployed = match deployed_bots(&state.db, &user.user_id, q.directing_bot_id.as_deref()) {
        Ok(d) => d,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    // Found: agents each connected vendor account lists that no bot of the owner is bound to.
    let mut found: Vec<Value> = vec![];
    let mut errors = 0;
    let all = deployed_bots(&state.db, &user.user_id, None).unwrap_or_default();
    let accounts: Vec<(String, String)> = state
        .db
        .connect()
        .ok()
        .and_then(|c| {
            let mut q = c.prepare("SELECT id, vendor FROM provider_account_bindings WHERE owner = ?1 AND state = 'CONNECTED' AND auth_type IN ('browser_session','desktop_session','api_key','oauth')").ok()?;
            let rows = q.query_map(params![user.user_id], |r| Ok((r.get(0)?, r.get(1)?))).ok()?.filter_map(Result::ok).collect();
            Some(rows)
        })
        .unwrap_or_default();
    let tx = crate::gateway_runner::transport(&state);
    for (aid, vendor) in accounts {
        let res = tokio::time::timeout(std::time::Duration::from_secs(8), crate::agent_gateway_routes::discover_agents(&state.db, tx.as_ref(), &user.user_id, &aid)).await;
        match res {
            Ok(Ok(agents)) => {
                for a in agents {
                    let ext = a["externalAgentId"].as_str().unwrap_or_default();
                    let taken = all.iter().any(|d| d["accountBindingId"] == aid.as_str() && d["externalAgentId"] == ext);
                    if !taken {
                        found.push(json!({ "accountBindingId": aid, "vendor": vendor, "externalAgentId": ext, "name": a["name"], "avatarUrl": a["avatarUrl"], "kind": a["kind"] }));
                    }
                }
            }
            _ => errors += 1,
        }
    }
    Json(json!({ "deployed": deployed, "found": found, "discoveryErrors": errors })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn facts(local: bool, web: bool, conn: bool, key: bool) -> LaneFacts {
        LaneFacts { local_app: local.then(|| "codex".to_string()), website_connected: web, connector_connected: conn, api_key_ready: key }
    }

    #[test]
    fn the_highest_available_lane_wins() {
        assert_eq!(pick_lane(&facts(true, true, true, true)), Some(LANE_LOCAL));
        assert_eq!(pick_lane(&facts(false, true, true, true)), Some(LANE_WEBSITE_CONNECTOR));
        assert_eq!(pick_lane(&facts(false, true, false, true)), Some(LANE_WEBSITE_ONLY));
        assert_eq!(pick_lane(&facts(false, false, true, true)), Some(LANE_API_KEY));
        assert_eq!(pick_lane(&facts(false, false, true, false)), None, "a connector without a signed-in site is not a lane");
        assert_eq!(pick_lane(&LaneFacts::default()), None);
        let v = lanes_view(&facts(false, true, false, false));
        assert_eq!(v["selected"], LANE_WEBSITE_ONLY);
        let avail: Vec<bool> = v["lanes"].as_array().unwrap().iter().map(|l| l["available"].as_bool().unwrap()).collect();
        assert_eq!(avail, [false, false, true, false]);
        assert_eq!(v["lanes"][0]["rank"], 1);
    }

    #[test]
    fn nudge_is_one_line_with_a_connector_and_the_full_task_without() {
        let t = json!({ "id": "T-7", "instructions": "Summarise the Q3 deck.", "allowedTools": ["read_thread", "send_email"] });
        for lane in [LANE_LOCAL, LANE_WEBSITE_CONNECTOR, LANE_API_KEY] {
            let n = nudge(lane, &t, "http://127.0.0.1:8013/mcp/bots/vb");
            assert!(n.starts_with("Run Allternit ticket T-7."), "{lane}: {n}");
            assert!(n.contains("get_ticket") && n.contains("post_result") && n.contains("/mcp/bots/vb"));
            assert!(!n.contains("Q3 deck") && !n.contains('\n'), "the structured part stays in the connector");
        }
        let full = nudge(LANE_WEBSITE_ONLY, &t, "u");
        assert!(full.contains("T-7") && full.contains("Summarise the Q3 deck.") && full.contains("read_thread, send_email"));
        assert!(!full.contains("post_result"), "this lane has no connector");
    }

    #[derive(Default)]
    struct Fake {
        local: Mutex<Vec<(String, String, String)>>,
        vendor: Mutex<Vec<String>>,
        reply: Mutex<Option<String>>,
        fail: Mutex<Option<String>>,
        /// Post this result through the connector while the turn runs.
        post: Mutex<Option<(DbHandle, String)>>,
    }
    #[async_trait]
    impl Sender for Fake {
        async fn local(&self, app: &str, server: &str, prompt: &str, _t: u64) -> Result<String, String> {
            self.local.lock().unwrap().push((app.into(), server.into(), prompt.into()));
            self.fail.lock().unwrap().clone().map_or_else(|| Ok(self.reply.lock().unwrap().clone().unwrap_or_default()), Err)
        }
        async fn vendor(&self, _o: &str, _th: &str, id: &str, text: &str) -> Result<Option<String>, String> {
            self.vendor.lock().unwrap().push(text.into());
            if let Some((db, bot)) = self.post.lock().unwrap().clone() {
                tool_post_result(&db, "user-a", &bot, &json!({ "id": id, "summary": "posted via connector", "data": { "rows": 3 } })).unwrap();
            }
            self.fail.lock().unwrap().clone().map_or_else(|| Ok(self.reply.lock().unwrap().clone()), Err)
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        crate::aai_facade::test_util::setup(tag, "READY").await
    }

    /// An account for vendor `vendor` bound to bot-vendor.
    fn account(st: &AppState, vendor: &str, auth: &str, state: &str, secret: bool) {
        let c = st.db.connect().unwrap();
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, secret_ref, state) VALUES ('acct-v','user-a',?1,?2,'ext',?3,?4)",
            params![vendor, auth, secret.then_some("enc:v1:x"), state],
        )
        .unwrap();
        c.execute("UPDATE bot_execution_bindings SET account_binding_id = 'acct-v' WHERE bot_id = 'bot-vendor'", []).unwrap();
    }

    fn ticket(st: &AppState) -> Value {
        create_ticket(&st.db, NewTicket { owner: "user-a", vendor_bot_id: "bot-vendor", directing_bot_id: Some("bot-native"), thread_id: "th-vendor", instructions: "Do the thing", allowed_tools: vec!["read_thread".into()], deadline_secs: Some(60), node: None }).unwrap()
    }

    fn status(st: &AppState, id: &str) -> Value {
        get_ticket(&st.db, "user-a", id).unwrap().unwrap()
    }

    fn cards(st: &AppState) -> i64 {
        st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE event_type = 'vendor.ticket.result'", [], |r| r.get(0)).unwrap()
    }

    fn expire(st: &AppState, id: &str) {
        st.db.connect().unwrap().execute("UPDATE vendor_tickets SET deadline_at = '2000-01-01T00:00:00Z' WHERE id = ?1", params![id]).unwrap();
    }

    #[tokio::test]
    async fn tickets_are_numbered_per_owner_and_validated() {
        let st = setup("tk-create").await;
        let a = ticket(&st);
        let b = ticket(&st);
        assert_eq!((a["id"].as_str(), b["id"].as_str()), (Some("T-1"), Some("T-2")));
        assert_eq!((a["status"].as_str(), a["directingBotId"].as_str(), a["allowedTools"].clone()), (Some("open"), Some("bot-native"), json!(["read_thread"])));
        let bad = |instructions: &str, thread: &str| create_ticket(&st.db, NewTicket { owner: "user-a", vendor_bot_id: "bot-vendor", directing_bot_id: None, thread_id: thread, instructions, allowed_tools: vec![], deadline_secs: None, node: None });
        assert!(bad("  ", "th-vendor").is_err());
        assert!(bad("x", "nope").is_err(), "someone else's or unknown thread");
        assert!(bad(&"x".repeat(MAX_INSTRUCTIONS + 1), "th-vendor").is_err());
        // deadline is clamped
        let c = create_ticket(&st.db, NewTicket { owner: "user-a", vendor_bot_id: "bot-vendor", directing_bot_id: None, thread_id: "th-vendor", instructions: "x", allowed_tools: vec![], deadline_secs: Some(1), node: None }).unwrap();
        let secs = (chrono::DateTime::parse_from_rfc3339(c["deadlineAt"].as_str().unwrap()).unwrap() - chrono::DateTime::parse_from_rfc3339(c["createdAt"].as_str().unwrap()).unwrap()).num_seconds();
        assert_eq!(secs, 30);
    }

    #[tokio::test]
    async fn connector_tools_get_post_and_list() {
        let st = setup("tk-tools").await;
        let t = ticket(&st);
        let id = t["id"].as_str().unwrap();
        let got = tool_get_ticket(&st.db, "user-a", "bot-vendor", &json!({ "id": id })).unwrap();
        assert_eq!(got["instructions"], "Do the thing");
        assert!(tool_get_ticket(&st.db, "user-a", "bot-other", &json!({ "id": id })).is_err(), "another vendor bot's ticket reads like no ticket");
        assert!(tool_get_ticket(&st.db, "user-b", "bot-vendor", &json!({ "id": id })).is_err(), "another owner's too");
        assert_eq!(tool_list_open_tickets(&st.db, "user-a", "bot-vendor").unwrap()["tickets"].as_array().unwrap().len(), 1);
        // validation
        assert!(tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": id })).is_err());
        assert!(tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": id, "summary": "s", "attachments": [{ "url": "http://x" }] })).is_err());
        assert!(tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": id, "summary": "s", "data": { "x": "y".repeat(MAX_DATA_BYTES) } })).is_err());
        assert_eq!(cards(&st), 0);
        let done = tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": id, "summary": "All done", "data": { "rows": 2 }, "attachments": [{ "name": "a", "url": "https://x.test/a.pdf" }] })).unwrap();
        assert_eq!((done["ticket"]["status"].as_str(), done["ticket"]["resultVia"].as_str()), (Some("done"), Some("connector")));
        assert_eq!(done["ticket"]["result"]["data"]["rows"], 2);
        assert_eq!(cards(&st), 1, "one result card in the thread");
        assert!(tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": id, "summary": "again" })).is_err(), "a ticket takes one result");
        assert_eq!(cards(&st), 1);
        assert!(tool_list_open_tickets(&st.db, "user-a", "bot-vendor").unwrap()["tickets"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn website_only_sends_the_full_task_and_the_reply_is_the_result() {
        let st = setup("tk-l3").await;
        account(&st, "xai", "browser_session", "CONNECTED", false);
        let t = ticket(&st);
        let f = Fake::default();
        *f.reply.lock().unwrap() = Some("Here is the answer".into());
        let out = dispatch(&st.db, &f, "user-a", t["id"].as_str().unwrap()).await.unwrap();
        assert_eq!((out["lane"].as_str(), out["status"].as_str(), out["resultVia"].as_str()), (Some(LANE_WEBSITE_ONLY), Some("done"), Some("reply")));
        assert_eq!(out["result"]["summary"], "Here is the answer");
        let sent = f.vendor.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("Do the thing") && !sent[0].contains("post_result"));
        assert_eq!(cards(&st), 1);
    }

    #[tokio::test]
    async fn connector_lanes_send_only_the_nudge_and_wait_for_post_result() {
        let st = setup("tk-l2").await;
        account(&st, "anthropic", "browser_session", "CONNECTED", false);
        st.db.connect().unwrap().execute("INSERT INTO vendor_connector_clients (id, vendor_bot_id, owner, client) VALUES ('c1','bot-vendor','user-a','claude-connector')", []).unwrap();
        let t = ticket(&st);
        let id = t["id"].as_str().unwrap();
        let f = Fake::default();
        *f.post.lock().unwrap() = Some((st.db.clone(), "bot-vendor".into()));
        *f.reply.lock().unwrap() = Some("chatty reply".into());
        let out = dispatch(&st.db, &f, "user-a", id).await.unwrap();
        assert_eq!((out["lane"].as_str(), out["status"].as_str(), out["resultVia"].as_str()), (Some(LANE_WEBSITE_CONNECTOR), Some("done"), Some("connector")));
        assert_eq!(out["result"]["summary"], "posted via connector", "the connector's result wins over the chat reply");
        let sent = f.vendor.lock().unwrap().clone();
        assert!(sent[0].starts_with(&format!("Run Allternit ticket {id}.")) && !sent[0].contains("Do the thing"));
        assert_eq!(cards(&st), 1);
    }

    #[tokio::test]
    async fn no_post_result_by_the_deadline_falls_back_to_the_reply_or_expires() {
        let st = setup("tk-fallback").await;
        account(&st, "anthropic", "browser_session", "CONNECTED", false);
        st.db.connect().unwrap().execute("INSERT INTO vendor_connector_clients (id, vendor_bot_id, owner, client) VALUES ('c1','bot-vendor','user-a','claude-connector')", []).unwrap();
        // with a reply
        let a = ticket(&st);
        let f = Fake::default();
        *f.reply.lock().unwrap() = Some("the reply text".into());
        let out = dispatch(&st.db, &f, "user-a", a["id"].as_str().unwrap()).await.unwrap();
        assert_eq!(out["status"], "sent", "still waiting on the connector");
        assert!(settle_deadlines(&st.db, Some("user-a"), &now()).is_empty(), "not due yet");
        expire(&st, "T-1");
        assert_eq!(settle_deadlines(&st.db, Some("user-a"), &now()), [("user-a".to_string(), "T-1".to_string())]);
        let done = status(&st, "T-1");
        assert_eq!((done["status"].as_str(), done["resultVia"].as_str(), done["result"]["summary"].as_str()), (Some("done"), Some("reply"), Some("the reply text")));
        assert_eq!(cards(&st), 1);
        // without one
        let b = ticket(&st);
        let f2 = Fake::default();
        dispatch(&st.db, &f2, "user-a", b["id"].as_str().unwrap()).await.unwrap();
        expire(&st, "T-2");
        // the connector read settles it too
        let err = tool_get_ticket(&st.db, "user-a", "bot-vendor", &json!({ "id": "T-2" })).unwrap();
        assert_eq!((err["status"].as_str(), err["error"].as_str().is_some()), (Some("expired"), true));
        assert!(tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": "T-2", "summary": "late" })).is_err(), "an expired ticket takes no result");
    }

    #[tokio::test]
    async fn local_app_lane_runs_the_app_and_needs_a_connected_runnable_app() {
        let st = setup("tk-l1").await;
        account(&st, "openai", "browser_session", "CONNECTED", false);
        // No connected app: falls to the website lane.
        assert_eq!(pick_lane(&lane_facts(&st.db, "user-a", "acct-v", Some("bot-vendor")).unwrap()), Some(LANE_WEBSITE_ONLY));
        let put = |app: &str, state: &str| {
            st.db.connect().unwrap().execute(
                "INSERT OR REPLACE INTO vendor_local_connectors (owner, app, vendor_bot_id, server_name, state, connected_at) VALUES ('user-a',?1,'bot-vendor','allternit-botvendor',?2,'2026')",
                params![app, state],
            ).unwrap();
        };
        put("codex", "unhealthy");
        assert_eq!(pick_lane(&lane_facts(&st.db, "user-a", "acct-v", None).unwrap()), Some(LANE_WEBSITE_ONLY), "an unhealthy app is not a lane");
        put("codex", "connected");
        let facts = lane_facts(&st.db, "user-a", "acct-v", None).unwrap();
        assert_eq!((pick_lane(&facts), facts.local_app.as_deref()), (Some(LANE_LOCAL), Some("codex")));
        let t = ticket(&st);
        let f = Fake::default();
        *f.reply.lock().unwrap() = Some("codex output".into());
        let out = dispatch(&st.db, &f, "user-a", t["id"].as_str().unwrap()).await.unwrap();
        assert_eq!((out["lane"].as_str(), out["status"].as_str(), out["resultVia"].as_str()), (Some(LANE_LOCAL), Some("done"), Some("reply")));
        let calls = f.local.lock().unwrap().clone();
        assert_eq!((calls[0].0.as_str(), calls[0].1.as_str()), ("codex", "allternit-botvendor"));
        assert!(calls[0].2.starts_with("Run Allternit ticket T-1.") && f.vendor.lock().unwrap().is_empty());
        // an account hosted on a cloud computer has no local app
        st.db.connect().unwrap().execute("UPDATE provider_account_bindings SET host_kind = 'cloud_computer' WHERE id = 'acct-v'", []).unwrap();
        assert_eq!(lane_facts(&st.db, "user-a", "acct-v", None).unwrap().local_app, None);
        // Claude Desktop alone is not runnable
        st.db.connect().unwrap().execute("UPDATE provider_account_bindings SET host_kind = 'this_device', vendor = 'anthropic' WHERE id = 'acct-v'", []).unwrap();
        put("claude_desktop", "connected");
        assert_eq!(lane_facts(&st.db, "user-a", "acct-v", None).unwrap().local_app, None);
    }

    #[tokio::test]
    async fn api_key_lane_and_no_lane() {
        let st = setup("tk-l4").await;
        account(&st, "xai", "api_key", "CONNECTED", true);
        assert_eq!(pick_lane(&lane_facts(&st.db, "user-a", "acct-v", None).unwrap()), Some(LANE_API_KEY));
        let t = ticket(&st);
        let f = Fake::default();
        let out = dispatch(&st.db, &f, "user-a", t["id"].as_str().unwrap()).await.unwrap();
        assert_eq!((out["lane"].as_str(), out["status"].as_str()), (Some(LANE_API_KEY), Some("sent")));
        assert!(f.vendor.lock().unwrap()[0].starts_with("Run Allternit ticket T-1."));
        // a signed-out account has no lane: the ticket fails plainly, nothing is sent
        st.db.connect().unwrap().execute("UPDATE provider_account_bindings SET state = 'EXPIRED'", []).unwrap();
        let t2 = ticket(&st);
        let f2 = Fake::default();
        let out = dispatch(&st.db, &f2, "user-a", t2["id"].as_str().unwrap()).await.unwrap();
        assert_eq!(out["status"], "failed");
        assert!(out["error"].as_str().unwrap().contains("No lane"));
        assert!(f2.vendor.lock().unwrap().is_empty() && f2.local.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_send_fails_the_ticket_and_dispatch_is_once_only() {
        let st = setup("tk-fail").await;
        account(&st, "xai", "browser_session", "CONNECTED", false);
        let t = ticket(&st);
        let f = Fake::default();
        *f.fail.lock().unwrap() = Some("vendor said no".into());
        let out = dispatch(&st.db, &f, "user-a", t["id"].as_str().unwrap()).await.unwrap();
        assert_eq!((out["status"].as_str(), out["error"].as_str()), (Some("failed"), Some("vendor said no")));
        dispatch(&st.db, &f, "user-a", t["id"].as_str().unwrap()).await.unwrap();
        assert_eq!(f.vendor.lock().unwrap().len(), 1, "a ticket that left 'open' is never sent twice");
        let c = ticket(&st);
        assert!(cancel_ticket(&st.db, "user-a", c["id"].as_str().unwrap()));
        assert_eq!(status(&st, "T-2")["status"], "cancelled");
        assert!(!cancel_ticket(&st.db, "user-a", "T-1"), "a failed ticket can't be cancelled");
    }

    #[tokio::test]
    async fn threads_record_which_vendor_bots_took_part() {
        let st = setup("tk-threads").await;
        assert!(participated_threads(&st.db, "user-a", "bot-vendor").unwrap().is_empty());
        let t = ticket(&st);
        let ths = participated_threads(&st.db, "user-a", "bot-vendor").unwrap();
        assert_eq!((ths.len(), ths[0]["threadId"].as_str(), ths[0]["source"].as_str()), (1, Some("th-vendor"), Some("ticket")));
        // a tool call through the connector on another thread
        let db = st.db.clone();
        crate::gateway_runner::led(&db, "bot-native", "th-native", None, "channel.message.received", ("user", "u"), json!({ "text": "hi" }), None);
        db.connect().unwrap().execute("INSERT INTO vendor_bot_thread_shares (vendor_bot_id, thread_id, owner) VALUES ('bot-vendor','th-native','user-a')", []).unwrap();
        let s = crate::mcp_vendor_bots::load_session(&db, "user-a", "bot-vendor").unwrap();
        struct NoActions;
        #[async_trait]
        impl crate::mcp_vendor_bots::Actions for NoActions {
            async fn send_text(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn start_call(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn send_email(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &str, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn post_message(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &Value, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn ask_bot(&self, _: &crate::mcp_vendor_bots::Session, _: &str) -> Result<Value, String> { Err("n".into()) }
        }
        crate::mcp_vendor_bots::call_tool(&db, &NoActions, &s, "read_thread", json!({ "threadId": "th-native" })).await;
        let ths = participated_threads(&st.db, "user-a", "bot-vendor").unwrap();
        assert_eq!(ths.len(), 2);
        assert!(ths.iter().any(|t| t["threadId"] == "th-native" && t["source"] == "tool:read_thread"));
        // a thread not visible to the bot records nothing and a stranger's thread never records
        crate::mcp_vendor_bots::call_tool(&db, &NoActions, &s, "get_ticket", json!({ "id": t["id"] })).await;
        record_participation(&db, "user-b", "bot-vendor", "th-vendor", "ticket");
        assert_eq!(participated_threads(&st.db, "user-a", "bot-vendor").unwrap().len(), 2);
        // the tool list carries the three new tools
        let names: Vec<String> = crate::mcp_vendor_bots::tool_descriptors().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        for n in ["get_ticket", "post_result", "list_open_tickets"] {
            assert!(names.contains(&n.to_string()), "{n}");
        }
    }

    #[tokio::test]
    async fn deployed_bots_follow_the_directing_bot() {
        let st = setup("tk-deployed").await;
        assert_eq!(deployed_bots(&st.db, "user-a", None).unwrap().len(), 1);
        assert!(deployed_bots(&st.db, "user-a", Some("bot-native")).unwrap().is_empty(), "no directing bot recorded yet");
        st.db.connect().unwrap().execute("UPDATE bot_execution_bindings SET directing_bot_id = 'bot-native' WHERE bot_id = 'bot-vendor'", []).unwrap();
        let d = deployed_bots(&st.db, "user-a", Some("bot-native")).unwrap();
        assert_eq!((d.len(), d[0]["vendorBotId"].as_str(), d[0]["directingBotId"].as_str()), (1, Some("bot-vendor"), Some("bot-native")));
        assert!(deployed_bots(&st.db, "user-a", Some("someone-else")).unwrap().is_empty());
        assert!(deployed_bots(&st.db, "user-b", None).unwrap().is_empty());
        // the connector's own relation counts too
        st.db.connect().unwrap().execute("UPDATE bot_execution_bindings SET directing_bot_id = NULL", []).unwrap();
        st.db.connect().unwrap().execute("INSERT INTO vendor_bot_connectors (vendor_bot_id, owner, directing_bot_id) VALUES ('bot-vendor','user-a','bot-native')", []).unwrap();
        assert_eq!(deployed_bots(&st.db, "user-a", Some("bot-native")).unwrap().len(), 1);
    }
}
