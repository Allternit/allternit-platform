//! Factory bots (SPEC §9 "Bots in the Factory"; migration V239).
//!
//! Every agent in the Factory is a Bot: an `agents` row with one execution
//! binding (`bot_execution_bindings`). This module adds the two calls the
//! engine CLI needs on top of the gateway tables:
//!
//! * `POST /api/v1/factory/bots`: create a bot and set its binding in one
//!   call (`allternit-factory agents bot add <slug> --binding terminal
//!   --harness claude`). Idempotent on (owner, slug): a repeat returns the same
//!   bot (200) and applies the binding as a rebind (the bot id never changes).
//!   `GET /api/v1/factory/bots` lists the owner's Factory bots.
//! * `POST /api/v1/factory/node-tickets`: deliver a node to a **vendor** bot as
//!   a vendor ticket (`T-n`) linked to (dag, node, WIH, workspace root).
//!   Idempotent on (dag, node, WIH). `GET /api/v1/factory/node-tickets?dagId=&nodeId=`
//!   reads the link back. When the ticket completes, [`close_linked_node`]
//!   records the result as the node output and closes the WIH `DONE` through
//!   the workspace's Gate, so the judge runs as for any close. A refused close
//!   keeps the result and marks the ticket (`nodeClose.state = "failed"`).
//!
//! Binding JSON follows API.md `Agent.binding`: `type` is `hosted` (stored as
//! `allternit`), `terminal` or `vendor`.
//!
//! Workspace roots are server paths, so a node ticket only accepts one this
//! process may write: the API's own rails root, a root under
//! `ALLTERNIT_FACTORY_WORKSPACE_ROOTS` (path list), or, on a loopback-bound
//! API (Desktop / local), any existing workspace with a `.allternit` folder.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::core::types::LedgerQuery;
use allternit_factory_engine::wih::project_wih;
use allternit_factory_engine::{Gate, GateOptions, Leases, LeasesOptions, Ledger, LedgerOptions, ReceiptStore, ReceiptStoreOptions};
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::agent_gateway_routes::{binding_type_and_mode, one, upsert_exec_binding, PutExec, EXEC_COLS};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::vendor_tickets::{self as vt, NewTicket, NodeLink};
use crate::AppState;

const MAX_NAME: usize = 120;
const MAX_ROLE: usize = 80;
/// A `closing` claim older than this is taken to be from a process that died mid-close.
const STALE_CLOSE_SECS: i64 = 300;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/factory/bots", post(create_bot_h).get(list_bots_h))
        .route("/v1/factory/node-tickets", post(create_node_ticket_h).get(list_node_tickets_h))
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ─── Bots ──────────────────────────────────────────────────────────────────────

/// A slug: lowercase letters, digits, `-`, `_`; starts with a letter or digit; 1-64 chars.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// `Agent.binding` (API.md) from a `bot_execution_bindings` row as `rows()` returns it.
pub fn binding_view(row: &Value) -> Value {
    let ty = match row["type"].as_str().unwrap_or_default() {
        "allternit" => "hosted",
        other => other,
    };
    let mut v = json!({ "id": row["id"], "type": ty, "state": row["state"] });
    match ty {
        "terminal" => {
            v["harness"] = row["harness"].clone();
            v["machine"] = row["machine"].clone();
            v["paneId"] = row["paneId"].clone();
        }
        "vendor" => {
            v["vendor"] = row["vendor"].clone();
            v["mode"] = row["mode"].clone();
            v["lane"] = row["preferredLane"].clone();
            v["guarantee"] = row["capabilities"]["guarantee"].clone();
            v["directingBotId"] = row["directingBotId"].clone();
            v["accountBindingId"] = row["accountBindingId"].clone();
        }
        _ => {}
    }
    v
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BindingBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub harness: Option<String>,
    pub machine: Option<String>,
    pub pane_id: Option<String>,
    pub vendor: Option<String>,
    pub mode: Option<String>,
    pub lane: Option<String>,
    pub account_binding_id: Option<String>,
    pub external_agent_id: Option<String>,
    pub directing_bot_id: Option<String>,
}

impl BindingBody {
    fn to_put(&self) -> PutExec {
        PutExec {
            r#type: self.kind.clone(),
            mode: self.mode.clone(),
            vendor: self.vendor.clone(),
            account_binding_id: self.account_binding_id.clone(),
            preferred_lane: self.lane.clone(),
            external_agent_id: self.external_agent_id.clone(),
            directing_bot_id: self.directing_bot_id.clone(),
            harness: self.harness.clone(),
            machine: self.machine.clone(),
            pane_id: self.pane_id.clone(),
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBotBody {
    pub slug: String,
    pub name: String,
    pub role: Option<String>,
    pub binding: BindingBody,
    /// Accepted for clients that send one; (owner, slug) already makes the call idempotent.
    pub idempotency_key: Option<String>,
}

/// The bot behind (owner, slug), if it still exists.
fn bot_for_slug(conn: &Connection, owner: &str, slug: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT s.bot_id FROM factory_bot_slugs s JOIN agents a ON a.id = s.bot_id AND a.user_id = s.owner WHERE s.owner = ?1 AND s.slug = ?2",
        params![owner, slug],
        |r| r.get(0),
    )
    .optional()
}

/// Create (or find) the bot for (owner, slug) and apply its binding.
/// Returns (created, response JSON) or (status, error).
pub fn ensure_bot(db: &DbHandle, owner: &str, b: &CreateBotBody) -> Result<(bool, Value), (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    let db_err = |e: rusqlite::Error| {
        warn!(error = %e, "factory bots DB error");
        (StatusCode::INTERNAL_SERVER_ERROR, "database error".to_string())
    };
    let slug = b.slug.trim();
    if !valid_slug(slug) {
        return Err(bad("slug must be 1-64 lowercase letters, digits, '-' or '_'"));
    }
    let name = b.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(bad("name must be 1-120 characters"));
    }
    let role = b.role.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if role.is_some_and(|r| r.chars().count() > MAX_ROLE) {
        return Err(bad("role must be at most 80 characters"));
    }
    let put = b.binding.to_put();
    // Validate the binding before any row is written, so a bad binding never leaves a bare bot.
    binding_type_and_mode(&put).map_err(|e| {
        let (s, v) = e.into_parts();
        (s, v["error"].as_str().unwrap_or("invalid binding").to_string())
    })?;
    let conn = db.connect().map_err(db_err)?;
    let factory_cfg = json!({ "isBot": true, "factory": { "slug": slug, "role": role } });
    let (bot_id, created) = match bot_for_slug(&conn, owner, slug).map_err(db_err)? {
        Some(id) => {
            conn.execute(
                "UPDATE agents SET name = ?1, config = json_set(COALESCE(config, '{}'), '$.factory', json(?2)), updated_at = CURRENT_TIMESTAMP WHERE id = ?3 AND user_id = ?4",
                params![name, factory_cfg["factory"].to_string(), id, owner],
            )
            .map_err(db_err)?;
            (id, false)
        }
        None => {
            // A slug row whose bot was deleted is stale: drop it and create afresh.
            conn.execute("DELETE FROM factory_bot_slugs WHERE owner = ?1 AND slug = ?2", params![owner, slug]).map_err(db_err)?;
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO agents (id, user_id, name, description, type, model, provider, config, is_bot, enabled_modes)
                 VALUES (?1, ?2, ?3, ?4, 'worker', '', '', ?5, 1, '[\"chat\"]')",
                params![id, owner, name, role.map(|r| format!("Factory {r} bot")), factory_cfg.to_string()],
            )
            .map_err(db_err)?;
            let claimed = conn
                .execute("INSERT OR IGNORE INTO factory_bot_slugs (owner, slug, bot_id, created_at) VALUES (?1, ?2, ?3, ?4)", params![owner, slug, id, now()])
                .map_err(db_err)?;
            if claimed == 0 {
                // A concurrent call claimed the slug first: its bot is the answer.
                let _ = conn.execute("DELETE FROM agents WHERE id = ?1 AND user_id = ?2", params![id, owner]);
                let winner = bot_for_slug(&conn, owner, slug).map_err(db_err)?.ok_or((StatusCode::CONFLICT, "slug is being created; retry".to_string()))?;
                (winner, false)
            } else {
                (id, true)
            }
        }
    };
    upsert_exec_binding(db, &conn, owner, &bot_id, &put).map_err(|e| {
        let (s, v) = e.into_parts();
        (s, v["error"].as_str().unwrap_or("binding failed").to_string())
    })?;
    let row = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&bot_id, &owner])
        .map_err(db_err)?
        .unwrap_or(Value::Null);
    Ok((created, json!({ "bot": { "id": bot_id, "slug": slug, "name": name, "role": role }, "binding": binding_view(&row), "created": created })))
}

async fn create_bot_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<CreateBotBody>) -> Response {
    let db = state.db.clone();
    let owner = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || ensure_bot(&db, &owner, &b)).await;
    match res {
        Ok(Ok((created, v))) => {
            if created {
                ledger_bot_created(&state, &user.user_id, &v).await;
            }
            (if created { StatusCode::CREATED } else { StatusCode::OK }, Json(v)).into_response()
        }
        Ok(Err((s, m))) => err(s, m),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    }
}

/// The same `agent.created` record `POST /api/v1/agents` writes, for traceability.
async fn ledger_bot_created(state: &Arc<AppState>, owner: &str, v: &Value) {
    let ev = allternit_factory_engine::AllternitEvent {
        event_id: String::new(),
        ts: String::new(),
        actor: allternit_factory_engine::Actor { r#type: allternit_factory_engine::ActorType::User, id: owner.to_string() },
        scope: None,
        r#type: "agent.created".to_string(),
        payload: json!({ "agent_id": v["bot"]["id"], "name": v["bot"]["name"], "slug": v["bot"]["slug"], "binding": v["binding"]["type"], "source": "factory" }),
        provenance: None,
    };
    if let Err(e) = state.rails.ledger.append(ev).await {
        warn!("Failed to append agent.created ledger event: {}", e);
    }
}

pub fn list_bots(db: &DbHandle, owner: &str) -> rusqlite::Result<Vec<Value>> {
    let conn = db.connect()?;
    let mut q = conn.prepare(
        "SELECT s.slug, a.id, a.name, json_extract(a.config, '$.factory.role')
         FROM factory_bot_slugs s JOIN agents a ON a.id = s.bot_id AND a.user_id = s.owner
         WHERE s.owner = ?1 ORDER BY s.slug",
    )?;
    let bots: Vec<(String, String, String, Option<String>)> =
        q.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
    let mut out = vec![];
    for (slug, id, name, role) in bots {
        let row = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&id, &owner])?;
        out.push(json!({ "id": id, "slug": slug, "name": name, "role": role, "binding": row.as_ref().map(binding_view) }));
    }
    Ok(out)
}

async fn list_bots_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || list_bots(&db, &user.user_id)).await {
        Ok(Ok(bots)) => Json(json!({ "bots": bots })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    }
}

// ─── Node tickets ──────────────────────────────────────────────────────────────

/// Which workspace roots this process may close WIHs in (see the module docs).
/// Returns the canonical path.
pub fn allowed_workspace_root(state: &AppState, raw: &str) -> Result<PathBuf, String> {
    let extra: Vec<PathBuf> = std::env::var_os("ALLTERNIT_FACTORY_WORKSPACE_ROOTS").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    check_workspace_root(raw, &state.rails.root_dir, &extra, state.config.api_host().is_loopback())
}

pub fn check_workspace_root(raw: &str, rails_root: &FsPath, extra_roots: &[PathBuf], local: bool) -> Result<PathBuf, String> {
    let p = PathBuf::from(raw.trim());
    if !p.is_absolute() {
        return Err("workspaceRoot must be an absolute path.".into());
    }
    let canon = p.canonicalize().map_err(|_| "workspaceRoot doesn't exist on this server.".to_string())?;
    if !canon.join(".allternit").is_dir() {
        return Err("workspaceRoot isn't a Factory workspace (no .allternit folder).".into());
    }
    let canon_of = |r: &FsPath| r.canonicalize().unwrap_or_else(|_| r.to_path_buf());
    if canon == canon_of(rails_root) || extra_roots.iter().any(|r| canon.starts_with(canon_of(r))) || local {
        Ok(canon)
    } else {
        Err("This server can't close nodes in that workspace. Set ALLTERNIT_FACTORY_WORKSPACE_ROOTS to allow it.".into())
    }
}

/// A Gate over `root`'s ledger, receipts and leases (no index, vault or renewal loop).
/// Opened per close: the ledger is append-only files, so this is safe beside the engine.
pub async fn open_gate(root: &FsPath) -> anyhow::Result<Gate> {
    let ledger = Arc::new(Ledger::new(LedgerOptions { root_dir: Some(root.to_path_buf()), ledger_dir: Some(PathBuf::from(".allternit/ledger")) }));
    let leases = Arc::new(
        Leases::new(LeasesOptions {
            root_dir: Some(root.to_path_buf()),
            leases_dir: Some(PathBuf::from(".allternit/leases")),
            event_sink: Some(ledger.clone()),
            actor_id: Some("api".to_string()),
            auto_renewal_enabled: false,
            ..Default::default()
        })
        .await?,
    );
    let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
        root_dir: Some(root.to_path_buf()),
        receipts_dir: Some(PathBuf::from(".allternit/receipts")),
        blobs_dir: Some(PathBuf::from(".allternit/blobs")),
    })?);
    Ok(Gate::new(GateOptions {
        ledger,
        leases,
        receipts,
        index: None,
        vault: None,
        oauth_vault: None,
        root_dir: Some(root.to_path_buf()),
        actor_id: Some("api".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    }))
}

/// The WIH in `root`'s ledger must exist, belong to (dag, node), and still be open.
pub async fn check_wih(root: &FsPath, dag_id: &str, node_id: &str, wih_id: &str) -> Result<(), String> {
    let ledger = Ledger::new(LedgerOptions { root_dir: Some(root.to_path_buf()), ledger_dir: Some(PathBuf::from(".allternit/ledger")) });
    let events = ledger.query(LedgerQuery::default()).await.map_err(|e| format!("Couldn't read the workspace ledger: {e}"))?;
    let w = project_wih(&events, wih_id).ok_or_else(|| format!("There's no WIH {wih_id} in that workspace."))?;
    if w.dag_id != dag_id || w.node_id != node_id {
        return Err(format!("WIH {wih_id} belongs to {}/{}, not {dag_id}/{node_id}.", w.dag_id, w.node_id));
    }
    if w.final_status.is_some() || w.closed_at.is_some() {
        return Err(format!("WIH {wih_id} is already closed."));
    }
    Ok(())
}

/// What the Gate is asked to do when a linked ticket completes.
#[derive(Debug, Clone, PartialEq)]
pub struct CloseRequest {
    pub workspace_root: PathBuf,
    pub dag_id: String,
    pub node_id: String,
    pub wih_id: String,
    /// Always `DONE`: the judge (if the node's policy asks for one) decides the final status.
    pub status: &'static str,
    pub evidence_refs: Vec<String>,
    /// The node output: the vendor's result as markdown.
    pub output: String,
}

/// Map a completed, node-linked ticket (as [`vt::get_ticket`] returns it) to its Gate close.
pub fn close_request(ticket: &Value) -> Result<CloseRequest, String> {
    let field = |k: &str| ticket[k].as_str().map(str::to_string).filter(|s| !s.is_empty()).ok_or_else(|| format!("ticket has no {k}"));
    if ticket["status"] != "done" {
        return Err(format!("ticket is {}, not done", ticket["status"].as_str().unwrap_or("unknown")));
    }
    let id = field("id")?;
    let result = &ticket["result"];
    let mut out = format!("Vendor ticket {id} result (via {}).\n\n", ticket["resultVia"].as_str().unwrap_or("connector"));
    out.push_str(result["summary"].as_str().unwrap_or("").trim());
    out.push('\n');
    if let Some(d) = result.get("data").filter(|d| !d.is_null()) {
        out.push_str(&format!("\n## Data\n\n```json\n{}\n```\n", serde_json::to_string_pretty(d).unwrap_or_default()));
    }
    if let Some(a) = result["attachments"].as_array().filter(|a| !a.is_empty()) {
        out.push_str("\n## Attachments\n\n");
        for x in a {
            let url = x["url"].as_str().unwrap_or("");
            let name = x["name"].as_str().unwrap_or(url);
            out.push_str(&format!("- [{name}]({url})\n"));
        }
    }
    Ok(CloseRequest {
        workspace_root: PathBuf::from(field("workspaceRoot")?),
        dag_id: field("dagId")?,
        node_id: field("nodeId")?,
        wih_id: field("wihId")?,
        status: "DONE",
        evidence_refs: vec![format!("vendor-ticket:{id}")],
        output: out,
    })
}

fn set_close_state(db: &DbHandle, owner: &str, id: &str, state: &str, error: Option<&str>, outcome: Option<&Value>) {
    if let Ok(c) = db.connect() {
        let _ = c.execute(
            "UPDATE vendor_tickets SET node_close_state = ?3, node_close_error = ?4, node_close_json = COALESCE(?5, node_close_json), node_close_at = ?6, updated_at = ?6 WHERE owner = ?1 AND id = ?2",
            params![owner, id, state, error, outcome.map(|o| o.to_string()), now()],
        );
    }
}

/// Close the node a completed ticket delivers: record its result as the node output and close
/// the WIH `DONE` through the workspace Gate. Runs at most once per ticket (claimed atomically);
/// returns the ticket as it stands afterwards. A refused close keeps the ticket's result and
/// marks `nodeClose.state = "failed"` with the reason.
pub async fn close_linked_node(db: &DbHandle, owner: &str, id: &str) -> Result<Value, String> {
    let stale = (chrono::Utc::now() - chrono::Duration::seconds(STALE_CLOSE_SECS)).to_rfc3339();
    let claimed = db
        .connect()
        .map_err(|e| e.to_string())?
        .execute(
            "UPDATE vendor_tickets SET node_close_state = 'closing', node_close_at = ?3
             WHERE owner = ?1 AND id = ?2 AND dag_id IS NOT NULL AND status = 'done'
               AND (node_close_state = 'pending' OR (node_close_state = 'closing' AND node_close_at < ?4))",
            params![owner, id, now(), stale],
        )
        .map_err(|e| e.to_string())?;
    let ticket = vt::get_ticket(db, owner, id)?.ok_or("There's no such ticket.")?;
    if claimed == 0 {
        return Ok(ticket);
    }
    let fail = |e: String| {
        warn!(owner = %owner, ticket = %id, error = %e, "vendor ticket result could not close its Factory node");
        set_close_state(db, owner, id, "failed", Some(&e), None);
    };
    let req = match close_request(&ticket) {
        Ok(r) => r,
        Err(e) => {
            fail(e);
            return vt::get_ticket(db, owner, id)?.ok_or_else(|| "ticket missing".into());
        }
    };
    let res = async {
        check_wih(&req.workspace_root, &req.dag_id, &req.node_id, &req.wih_id).await?;
        let gate = open_gate(&req.workspace_root).await.map_err(|e| format!("Couldn't open the workspace Gate: {e}"))?;
        gate.wih_close_as(&req.wih_id, req.status, &req.evidence_refs, Some(&req.output), None).await.map_err(|e| format!("The Gate refused the close: {e}"))
    }
    .await;
    match res {
        Ok(outcome) => {
            let v = json!({ "receiptId": outcome.output_receipt_id, "finalStatus": outcome.final_status, "nodeStatus": outcome.node_status, "verdict": outcome.verdict });
            set_close_state(db, owner, id, "closed", None, Some(&v));
        }
        Err(e) => fail(e),
    }
    vt::get_ticket(db, owner, id)?.ok_or_else(|| "ticket missing".into())
}

/// Close a linked ticket's node in the background (used where the completion path is sync).
/// Without a runtime the ticket stays `pending` and the next node-ticket read retries it.
pub fn spawn_node_close(db: DbHandle, owner: String, id: String) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(async move {
                if let Err(e) = close_linked_node(&db, &owner, &id).await {
                    warn!(ticket = %id, error = %e, "node close failed to run");
                }
            });
        }
        Err(_) => warn!(ticket = %id, "no async runtime; node close left pending"),
    }
}

/// Retry every linked ticket of `owner` whose close never ran (process restart, no runtime).
pub async fn retry_pending_closes(db: &DbHandle, owner: &str) {
    let stale = (chrono::Utc::now() - chrono::Duration::seconds(STALE_CLOSE_SECS)).to_rfc3339();
    let ids: Vec<String> = db
        .connect()
        .ok()
        .and_then(|c| {
            let mut q = c
                .prepare("SELECT id FROM vendor_tickets WHERE owner = ?1 AND status = 'done' AND (node_close_state = 'pending' OR (node_close_state = 'closing' AND node_close_at < ?2)) LIMIT 20")
                .ok()?;
            let rows = q.query_map(params![owner, stale], |r| r.get(0)).ok()?.filter_map(Result::ok).collect();
            Some(rows)
        })
        .unwrap_or_default();
    for id in ids {
        let _ = close_linked_node(db, owner, &id).await;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTicketBody {
    pub bot_id: Option<String>,
    pub bot_slug: Option<String>,
    pub dag_id: String,
    pub node_id: String,
    pub wih_id: String,
    pub workspace_root: String,
    pub title: String,
    pub instructions: String,
    /// Seconds from now, or an RFC 3339 time.
    pub deadline: Option<Value>,
    /// The vendor bot's thread to deliver in (default: its most recent thread).
    pub thread_id: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Create the ticket without sending it.
    #[serde(default)]
    pub hold: bool,
}

fn deadline_secs(v: &Option<Value>) -> Result<Option<i64>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_i64().map(Some).ok_or_else(|| "deadline must be whole seconds or an RFC 3339 time.".into()),
        Some(Value::String(s)) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| Some((d.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds()))
            .map_err(|_| "deadline must be whole seconds or an RFC 3339 time.".into()),
        _ => Err("deadline must be whole seconds or an RFC 3339 time.".into()),
    }
}

/// Resolve the target bot (id or slug) for `owner`.
fn resolve_bot(db: &DbHandle, owner: &str, b: &NodeTicketBody) -> Result<String, (StatusCode, String)> {
    let conn = db.connect().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".to_string()))?;
    let found: Option<String> = match (b.bot_id.as_deref().map(str::trim).filter(|s| !s.is_empty()), b.bot_slug.as_deref().map(str::trim).filter(|s| !s.is_empty())) {
        (Some(id), _) => conn.query_row("SELECT id FROM agents WHERE id = ?1 AND user_id = ?2", params![id, owner], |r| r.get(0)).optional().ok().flatten(),
        (None, Some(slug)) => bot_for_slug(&conn, owner, slug).ok().flatten(),
        (None, None) => return Err((StatusCode::BAD_REQUEST, "botId or botSlug is required.".into())),
    };
    found.ok_or((StatusCode::NOT_FOUND, "There's no such bot.".into()))
}

fn latest_thread(db: &DbHandle, owner: &str, bot_id: &str) -> Option<String> {
    db.connect()
        .ok()?
        .query_row("SELECT id FROM bot_threads WHERE user_id = ?1 AND bot_id = ?2 ORDER BY last_activity_at DESC LIMIT 1", params![owner, bot_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

/// The lane a new ticket for `vendor_bot_id` would take now (what [`vt::dispatch`] picks).
fn current_lane(db: &DbHandle, owner: &str, vendor_bot_id: &str) -> Option<&'static str> {
    let account: Option<String> = db
        .connect()
        .ok()?
        .query_row("SELECT account_binding_id FROM bot_execution_bindings WHERE owner = ?1 AND bot_id = ?2 AND type = 'vendor'", params![owner, vendor_bot_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
        .flatten();
    let facts = vt::lane_facts(db, owner, &account?, Some(vendor_bot_id)).ok()?;
    vt::pick_lane(&facts)
}

/// Create (or find) the node ticket. Returns (created, ticket) or (status, error).
pub async fn ensure_node_ticket(state: &AppState, owner: &str, b: &NodeTicketBody) -> Result<(bool, Value), (StatusCode, String)> {
    let bad = |m: String| (StatusCode::BAD_REQUEST, m);
    for (k, v) in [("dagId", &b.dag_id), ("nodeId", &b.node_id), ("wihId", &b.wih_id), ("workspaceRoot", &b.workspace_root), ("title", &b.title)] {
        if v.trim().is_empty() {
            return Err(bad(format!("{k} is required.")));
        }
    }
    let bot = resolve_bot(&state.db, owner, b)?;
    if let Some(t) = vt::ticket_for_node(&state.db, owner, b.dag_id.trim(), b.node_id.trim(), b.wih_id.trim()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))? {
        return Ok((false, t));
    }
    if crate::mcp_vendor_bots::load_session(&state.db, owner, &bot).is_none() {
        return Err((StatusCode::CONFLICT, "That bot isn't a vendor bot. Hosted and terminal bots pick nodes up directly; only vendor bots take tickets.".into()));
    }
    let root = allowed_workspace_root(state, &b.workspace_root).map_err(bad)?;
    check_wih(&root, b.dag_id.trim(), b.node_id.trim(), b.wih_id.trim()).await.map_err(bad)?;
    let thread = match b.thread_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => latest_thread(&state.db, owner, &bot).ok_or_else(|| bad("That vendor bot has no thread yet. Open one, or pass threadId.".into()))?,
    };
    let secs = deadline_secs(&b.deadline).map_err(bad)?;
    let instructions = format!("{}\n\n{}", b.title.trim(), b.instructions.trim());
    let root_s = root.to_string_lossy().to_string();
    let directing: Option<String> = state
        .db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT directing_bot_id FROM bot_execution_bindings WHERE owner = ?1 AND bot_id = ?2", params![owner, bot], |r| r.get(0)).optional().ok().flatten().flatten());
    let t = vt::create_ticket(
        &state.db,
        NewTicket {
            owner,
            vendor_bot_id: &bot,
            directing_bot_id: directing.as_deref(),
            thread_id: &thread,
            instructions: instructions.trim(),
            allowed_tools: b.allowed_tools.clone(),
            deadline_secs: secs,
            node: Some(NodeLink { dag_id: b.dag_id.trim(), node_id: b.node_id.trim(), wih_id: b.wih_id.trim(), workspace_root: &root_s }),
        },
    )
    .map_err(bad)?;
    // create_ticket returns a concurrent winner unchanged; only a fresh ticket is still 'open' with no lane.
    let created = t["status"] == "open" && t["lane"].is_null();
    Ok((created, t))
}

async fn create_node_ticket_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<NodeTicketBody>) -> Response {
    let (created, t) = match ensure_node_ticket(&state, &user.user_id, &b).await {
        Ok(x) => x,
        Err((s, m)) => return err(s, m),
    };
    let id = t["id"].as_str().unwrap_or_default().to_string();
    let (lane, nudge_sent) = if created {
        let lane = current_lane(&state.db, &user.user_id, t["vendorBotId"].as_str().unwrap_or_default());
        let send = !b.hold && lane.is_some();
        if !b.hold {
            // With no lane, dispatch fails the ticket plainly ("No lane is available").
            vt::spawn_dispatch(&state, &user.user_id, &t);
        }
        (lane.map(str::to_string), send)
    } else {
        (t["lane"].as_str().map(str::to_string), t["status"] != "open")
    };
    let body = json!({
        "ticket": id, "ticketId": id, "lane": lane, "guarantee": lane.as_deref().map(vt::lane_guarantee),
        "nudgeSent": nudge_sent, "created": created, "record": t,
    });
    (if created { StatusCode::CREATED } else { StatusCode::OK }, Json(body)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeTicketQuery {
    dag_id: Option<String>,
    node_id: Option<String>,
}

async fn list_node_tickets_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<NodeTicketQuery>) -> Response {
    let dag = q.dag_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let node = q.node_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if dag.is_none() {
        return err(StatusCode::BAD_REQUEST, "dagId is required.");
    }
    vt::settle_deadlines(&state.db, Some(&user.user_id), &now());
    retry_pending_closes(&state.db, &user.user_id).await;
    match vt::tickets_for_node(&state.db, &user.user_id, dag, node) {
        Ok(t) => Json(json!({ "tickets": t })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn user(id: &str) -> AuthUser {
        AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    async fn call(app: &Router, method: &str, uid: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .extension(user(uid))
            .body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let st = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn app(state: &Arc<AppState>) -> Router {
        Router::new()
            .nest("/api", router())
            .nest("/api/v1", crate::agent_gateway_routes::agent_gateway_router())
            .with_state(state.clone())
    }

    #[tokio::test]
    async fn terminal_bot_create_is_idempotent_and_reads_back() {
        let st = crate::aai_facade::test_util::setup("fb-create", "READY").await;
        let app = app(&st);
        // harness is required for a terminal binding, and no bot is left behind
        let (s, b) = call(&app, "POST", "user-a", "/api/v1/factory/bots", Some(json!({ "slug": "builder", "name": "Builder", "binding": { "type": "terminal" } }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        assert!(b["error"].as_str().unwrap().contains("harness"));
        assert!(list_bots(&st.db, "user-a").unwrap().is_empty());
        // vendor fields never ride on a terminal binding
        let (s, _) = call(&app, "POST", "user-a", "/api/v1/factory/bots", Some(json!({ "slug": "builder", "name": "Builder", "binding": { "type": "terminal", "harness": "claude", "lane": "official" } }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        let body = json!({ "slug": "builder", "name": "Builder", "role": "build", "binding": { "type": "terminal", "harness": "claude", "machine": "mac-1", "paneId": "%3" } });
        let (s, first) = call(&app, "POST", "user-a", "/api/v1/factory/bots", Some(body.clone())).await;
        assert_eq!(s, StatusCode::CREATED, "{first}");
        assert_eq!(first["binding"]["type"], "terminal");
        assert_eq!((first["binding"]["harness"].as_str(), first["binding"]["machine"].as_str(), first["binding"]["paneId"].as_str()), (Some("claude"), Some("mac-1"), Some("%3")));
        assert!(first["binding"].get("lane").is_none(), "no vendor fields on a terminal binding");
        let (s, again) = call(&app, "POST", "user-a", "/api/v1/factory/bots", Some(body)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(again["bot"]["id"], first["bot"]["id"], "same bot on a repeat");
        // another owner gets their own bot for the same slug
        let (s, theirs) = call(&app, "POST", "user-b", "/api/v1/factory/bots", Some(json!({ "slug": "builder", "name": "B", "binding": { "type": "hosted" } }))).await;
        assert_eq!(s, StatusCode::CREATED);
        assert_ne!(theirs["bot"]["id"], first["bot"]["id"]);
        assert_eq!(theirs["binding"]["type"], "hosted");

        // the existing gateway read shows the terminal binding with camelCase fields
        let bot = first["bot"]["id"].as_str().unwrap();
        let (s, read) = call(&app, "GET", "user-a", &format!("/api/v1/gateway/bots/{bot}/execution-binding"), None).await;
        assert_eq!(s, StatusCode::OK, "{read}");
        assert_eq!((read["binding"]["type"].as_str(), read["binding"]["harness"].as_str(), read["binding"]["paneId"].as_str()), (Some("terminal"), Some("claude"), Some("%3")));
        // the gateway PUT rejects a terminal binding without a harness too
        let (s, _) = call(&app, "PUT", "user-a", &format!("/api/v1/gateway/bots/{bot}/execution-binding"), Some(json!({ "type": "terminal" }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // a rebind to another harness keeps the bot and binding ids
        let (_, re) = call(&app, "POST", "user-a", "/api/v1/factory/bots", Some(json!({ "slug": "builder", "name": "Builder", "binding": { "type": "terminal", "harness": "codex" } }))).await;
        assert_eq!((re["bot"]["id"].clone(), re["binding"]["id"].clone(), re["binding"]["harness"].as_str()), (first["bot"]["id"].clone(), first["binding"]["id"].clone(), Some("codex")));
        assert_eq!(re["binding"]["paneId"], Value::Null);

        // a terminal bot is never a vendor bot: no connector session, no deployed listing, provenance says terminal
        assert!(crate::mcp_vendor_bots::load_session(&st.db, "user-a", bot).is_none());
        assert!(vt::deployed_bots(&st.db, "user-a", None).unwrap().iter().all(|d| d["vendorBotId"] != bot));
        let agents = crate::aai_facade::agents_list(&st, "user-a", None).unwrap();
        let p = &agents.iter().find(|a| a["id"] == bot).unwrap()["provenance"];
        assert_eq!((p["kind"].as_str(), p["harness"].as_str()), (Some("terminal"), Some("codex")));
        let (s, listed) = call(&app, "GET", "user-a", "/api/v1/factory/bots", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(listed["bots"].as_array().unwrap().len(), 1);
        assert_eq!(listed["bots"][0]["slug"], "builder");
    }

    /// A DAG with one node picked up as a WIH in the test rails workspace.
    async fn picked_up_node(st: &Arc<AppState>, tag: &str) -> (String, String, String) {
        let gate = &st.rails.gate;
        let (_p, dag, node) = gate.plan_new(&format!("Research {tag}"), None).await.unwrap();
        let wih = gate.wih_pickup(&dag, &node, "bot-vendor").await.unwrap();
        (dag, node, wih)
    }

    fn root(st: &AppState) -> String {
        st.rails.root_dir.canonicalize().unwrap().to_string_lossy().to_string()
    }

    #[tokio::test]
    async fn node_tickets_link_both_ways_and_are_idempotent() {
        let st = crate::aai_facade::test_util::setup("fb-nt", "READY").await;
        let app = app(&st);
        let (dag, node, wih) = picked_up_node(&st, "link").await;
        let body = json!({ "botId": "bot-vendor", "dagId": dag, "nodeId": node, "wihId": wih, "workspaceRoot": root(&st),
                           "title": "Find three sources", "instructions": "On the Q3 market.", "deadline": 120, "hold": true });
        let (s, a) = call(&app, "POST", "user-a", "/api/v1/factory/node-tickets", Some(body.clone())).await;
        assert_eq!(s, StatusCode::CREATED, "{a}");
        assert_eq!(a["ticket"], "T-1");
        assert_eq!(a["ticketId"], "T-1");
        assert_eq!(a["nudgeSent"], false, "held");
        assert_eq!((a["record"]["dagId"].as_str(), a["record"]["nodeId"].as_str(), a["record"]["wihId"].as_str()), (Some(dag.as_str()), Some(node.as_str()), Some(wih.as_str())));
        assert!(a["record"]["instructions"].as_str().unwrap().starts_with("Find three sources"));
        let (s, b) = call(&app, "POST", "user-a", "/api/v1/factory/node-tickets", Some(body.clone())).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["ticket"], "T-1", "same ticket for the same (dag, node, WIH)");
        // the ticket's own JSON carries the node; the node lists the ticket
        assert_eq!(vt::get_ticket(&st.db, "user-a", "T-1").unwrap().unwrap()["nodeId"], node.as_str());
        let (s, l) = call(&app, "GET", "user-a", &format!("/api/v1/factory/node-tickets?dagId={dag}&nodeId={node}"), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(l["tickets"].as_array().unwrap().len(), 1);
        let (_, other) = call(&app, "GET", "user-b", &format!("/api/v1/factory/node-tickets?dagId={dag}"), None).await;
        assert!(other["tickets"].as_array().unwrap().is_empty(), "owner-scoped");
        // refusals: a non-vendor bot, a WIH of another node, a workspace outside the allowed roots
        let mut nb = body.clone();
        nb["botId"] = json!("bot-native");
        nb["wihId"] = json!("wih-other");
        assert_eq!(call(&app, "POST", "user-a", "/api/v1/factory/node-tickets", Some(nb)).await.0, StatusCode::CONFLICT);
        let mut wrong = body.clone();
        wrong["nodeId"] = json!("n_nope");
        assert_eq!(call(&app, "POST", "user-a", "/api/v1/factory/node-tickets", Some(wrong)).await.0, StatusCode::BAD_REQUEST);
        let elsewhere = std::env::temp_dir().join(format!("fb-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(elsewhere.join(".allternit")).unwrap();
        assert!(check_workspace_root(elsewhere.to_str().unwrap(), &st.rails.root_dir, &[], false).is_err(), "a hosted server refuses other roots");
        assert!(check_workspace_root(elsewhere.to_str().unwrap(), &st.rails.root_dir, &[], true).is_ok(), "a local API accepts a real workspace");
        assert!(check_workspace_root(elsewhere.to_str().unwrap(), &st.rails.root_dir, &[elsewhere.clone()], false).is_ok());
        assert!(check_workspace_root("relative/path", &st.rails.root_dir, &[], true).is_err());
        // a plain ticket (no node) keeps its old shape: no node fields, no close
        let plain = vt::create_ticket(&st.db, NewTicket { owner: "user-a", vendor_bot_id: "bot-vendor", thread_id: "th-vendor", instructions: "plain", ..Default::default() }).unwrap();
        assert!(plain["dagId"].is_null() && plain["nodeClose"].is_null());
        let done = vt::tool_post_result(&st.db, "user-a", "bot-vendor", &json!({ "id": plain["id"], "summary": "ok" })).unwrap();
        assert!(done["ticket"]["nodeClose"].is_null());
    }

    #[tokio::test]
    async fn post_result_on_a_linked_ticket_closes_the_node_with_the_output() {
        let st = crate::aai_facade::test_util::setup("fb-close", "READY").await;
        let app = app(&st);
        let (dag, node, wih) = picked_up_node(&st, "close").await;
        let body = json!({ "botId": "bot-vendor", "dagId": dag, "nodeId": node, "wihId": wih, "workspaceRoot": root(&st), "title": "T", "instructions": "I", "hold": true });
        let (_, a) = call(&app, "POST", "user-a", "/api/v1/factory/node-tickets", Some(body)).await;
        let id = a["ticket"].as_str().unwrap().to_string();
        // through the connector exactly as a vendor would call it
        let s = crate::mcp_vendor_bots::load_session(&st.db, "user-a", "bot-vendor").unwrap();
        struct NoActions;
        #[async_trait::async_trait]
        impl crate::mcp_vendor_bots::Actions for NoActions {
            async fn send_text(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn start_call(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn send_email(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &str, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn post_message(&self, _: &crate::mcp_vendor_bots::Session, _: &str, _: &Value, _: &str) -> Result<Value, String> { Err("n".into()) }
            async fn ask_bot(&self, _: &crate::mcp_vendor_bots::Session, _: &str) -> Result<Value, String> { Err("n".into()) }
        }
        let out = crate::mcp_vendor_bots::call_tool(&st.db, &NoActions, &s, "post_result", json!({ "id": id, "summary": "Three sources found.", "data": { "n": 3 } })).await;
        assert_eq!(out["isError"], false, "{out}");
        let t = vt::get_ticket(&st.db, "user-a", &id).unwrap().unwrap();
        assert_eq!(t["status"], "done");
        assert_eq!(t["nodeClose"]["state"], "closed", "{t}");
        assert_eq!(t["nodeClose"]["finalStatus"], "DONE");
        let receipt = t["nodeClose"]["receiptId"].as_str().unwrap().to_string();
        assert_eq!(out["structuredContent"]["ticket"]["nodeClose"]["state"], "closed", "the vendor's answer says the node closed");
        // the node output is the result, behind a receipt
        let out_path = st.rails.root_dir.join(allternit_factory_engine::gate::gate::node_output_rel_path(&dag, &node));
        let text = std::fs::read_to_string(out_path).unwrap();
        assert!(text.contains("Three sources found.") && text.contains("\"n\": 3"), "{text}");
        let events = st.rails.ledger.query(LedgerQuery::default()).await.unwrap();
        assert!(events.iter().any(|e| e.r#type == "WIHClosedSigned" && e.payload["wih_id"] == wih.as_str()));
        assert!(events.iter().any(|e| e.r#type == "WIHCloseRequested" && e.payload["evidence_refs"].as_array().unwrap().iter().any(|r| r == &json!(format!("receipt:{receipt}")))));
        // closing again is a no-op
        let again = close_linked_node(&st.db, "user-a", &id).await.unwrap();
        assert_eq!(again["nodeClose"]["state"], "closed");
    }

    #[tokio::test]
    async fn a_refused_close_keeps_the_result_and_says_why() {
        let st = crate::aai_facade::test_util::setup("fb-refuse", "READY").await;
        let (dag, node, wih) = picked_up_node(&st, "refuse").await;
        let t = vt::create_ticket(
            &st.db,
            NewTicket { owner: "user-a", vendor_bot_id: "bot-vendor", thread_id: "th-vendor", instructions: "x", node: Some(NodeLink { dag_id: &dag, node_id: &node, wih_id: &wih, workspace_root: &root(&st) }), ..Default::default() },
        )
        .unwrap();
        let id = t["id"].as_str().unwrap();
        // someone else closed the WIH first
        st.rails.gate.wih_close_with(&wih, "DONE", &["elsewhere".to_string()], None).await.unwrap();
        vt::complete_ticket(&st.db, "user-a", id, json!({ "summary": "late result" }), "connector").unwrap();
        let after = close_linked_node(&st.db, "user-a", id).await.unwrap();
        assert_eq!((after["status"].as_str(), after["result"]["summary"].as_str()), (Some("done"), Some("late result")), "result kept");
        assert_eq!(after["nodeClose"]["state"], "failed");
        assert!(after["nodeClose"]["error"].as_str().unwrap().contains("already closed"), "{after}");
    }

    #[test]
    fn close_request_maps_a_ticket_to_a_done_close_with_markdown_output() {
        let t = json!({ "id": "T-4", "status": "done", "resultVia": "connector", "dagId": "d1", "nodeId": "n1", "wihId": "w1", "workspaceRoot": "/ws",
                        "result": { "summary": "Done.", "data": { "k": 1 }, "attachments": [{ "name": "a.pdf", "url": "https://x.test/a.pdf" }] } });
        let r = close_request(&t).unwrap();
        assert_eq!((r.status, r.dag_id.as_str(), r.node_id.as_str(), r.wih_id.as_str()), ("DONE", "d1", "n1", "w1"));
        assert_eq!(r.workspace_root, PathBuf::from("/ws"));
        assert_eq!(r.evidence_refs, vec!["vendor-ticket:T-4".to_string()]);
        assert!(r.output.contains("Done.") && r.output.contains("\"k\": 1") && r.output.contains("[a.pdf](https://x.test/a.pdf)"));
        let mut open = t.clone();
        open["status"] = json!("sent");
        assert!(close_request(&open).is_err());
        let mut unlinked = t;
        unlinked["wihId"] = Value::Null;
        assert!(close_request(&unlinked).is_err());
        assert!(valid_slug("builder-2") && !valid_slug("Builder") && !valid_slug("-x") && !valid_slug(""));
    }
}
