//! Team map read routes (Island bar + Team map program, lane 12). READ only:
//! writes (people, grants, rules) are lane 13.
//!
//! - `GET /team/map`: nodes (people, bots), edges, grants, rules (with a
//!   rendered `text`), asks, jobs, and `partial[]` (cloud sources that failed).
//! - `GET /team/approvals?role=approver|requester&state=pending`: the one read
//!   model over `cowork_approvals`, `factory_approvals` and the cloud routed
//!   list. `resolve` on each Ask is that store's own resolve route.
//! - `GET /team/apps/:kind/:ref/access`: who can use one app.
//!
//! Visibility: every member sees nodes, edges, levels and counts. Ask details
//! go only to the approver, the requester and an owner/admin.

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::account_sync::{AccountCloud, SignedAccountCloud};
use crate::auth::AuthUser;
use crate::team_store::{self, AppKind, ApprovalRule, ApproverKind, BotKind, GrantLevel, RuleOp, ScopeKind};
use crate::AppState;

/// Cloud routed approvals (`platform:` + `meeting:` rows addressed to this
/// runtime's owner). Lane 14b (cloud PR #2983) fixed the path; the response is
/// `{object:"list", data:[Ask-like rows]}`. Signed like `account_sync`.
pub const CLOUD_ROUTED_APPROVALS_PATH: &str = "/api/v1/approvals/routed?limit=100";

/// `partial[]` entry when the cloud routed list could not be read.
pub const PARTIAL_PLATFORM_APPROVALS: &str = "platform_approvals";
/// `partial[]` entry: grant ceilings come from the cloud `app_tiers` and are
/// not read here yet, so `ceiling` is the static one (vendor: app, else full).
pub const PARTIAL_APP_TIERS: &str = "app_tiers";

pub fn team_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/team/map", get(map_route))
        .route("/team/approvals", get(approvals_route))
        .route("/team/apps/:kind/:app_ref/access", get(access_route))
}

fn err(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

// ------------------------------------------------------------------ viewer

/// Who is asking, resolved against `organization_members`.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub user_id: String,
    pub org_id: Option<String>,
    /// owner or admin of the org (a solo account is its own owner).
    pub is_admin: bool,
    pub name: Option<String>,
    pub avatar: Option<String>,
}

pub fn resolve_viewer(conn: &Connection, user: &AuthUser) -> rusqlite::Result<Viewer> {
    let membership: Option<(String, String)> = match user.organization_id.as_deref() {
        Some(org) => conn
            .query_row(
                "SELECT organization_id, role FROM organization_members WHERE organization_id = ?1 AND user_id = ?2",
                params![org, user.user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?,
        None => conn
            .query_row(
                "SELECT organization_id, role FROM organization_members WHERE user_id = ?1 ORDER BY joined_at LIMIT 1",
                params![user.user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?,
    };
    let (org_id, role) = match membership {
        Some((o, r)) => (Some(o), r),
        None => (user.organization_id.clone(), user.organization_role.clone().unwrap_or_default()),
    };
    let role = role.to_ascii_lowercase();
    let role = role.trim_start_matches("org:");
    let is_admin = org_id.is_none() || role == "owner" || role == "admin";
    Ok(Viewer { user_id: user.user_id.clone(), org_id, is_admin, name: user.name.clone(), avatar: user.avatar_url.clone() })
}

// ------------------------------------------------------------------ asks

/// One pending ask with the facts visibility needs.
#[derive(Debug, Clone)]
struct AskRec {
    ask: Value,
    requester: Option<String>,
    approver: Option<String>,
    bot: Option<String>,
}

impl AskRec {
    fn visible_to(&self, v: &Viewer) -> bool {
        v.is_admin || self.requester.as_deref() == Some(&v.user_id) || self.approver.as_deref() == Some(&v.user_id)
    }
}

fn node_actor(bot: Option<&str>, user: Option<&str>) -> Value {
    match (bot, user) {
        (Some(b), _) => json!(format!("b:{b}")),
        (None, Some(u)) => json!(format!("p:{u}")),
        _ => Value::Null,
    }
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v[*k].as_str()).map(str::trim).filter(|s| !s.is_empty())
}

fn cowork_asks(conn: &Connection, scope: &[String]) -> rusqlite::Result<Vec<AskRec>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, content, source, created_at, approver_user_id, rule_id, bot_id, job_id
         FROM cowork_approvals WHERE dismissed = 0 AND decision IS NULL ORDER BY created_at DESC, id LIMIT 500",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut out = vec![];
    for row in rows {
        let (id, user_id, content, source, created_at, approver, rule_id, bot_id, job_id) = row?;
        // Unrouted rows (V294 columns NULL) are the requester's own ask.
        let effective_approver = approver.clone().or_else(|| user_id.clone());
        let in_scope = |u: &Option<String>| u.as_ref().is_some_and(|u| scope.contains(u));
        if !in_scope(&user_id) && !in_scope(&effective_approver) {
            continue;
        }
        let body: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
        let tool = str_field(&body, &["tool", "toolName", "tool_name"]).map(str::to_string);
        let title = str_field(&body, &["title", "summary", "description", "message"])
            .map(str::to_string)
            .or_else(|| tool.clone().map(|t| format!("Use {t}")))
            .unwrap_or_else(|| "Approval needed".to_string());
        let summary = str_field(&body, &["summary", "description", "message"]).unwrap_or("").to_string();
        out.push(AskRec {
            ask: json!({
                "id": format!("cowork:{id}"),
                "store": "cowork",
                "title": title,
                "summary": summary,
                "actor": node_actor(bot_id.as_deref(), user_id.as_deref()),
                "approver_user_id": effective_approver,
                "rule_id": rule_id,
                "app": Value::Null,
                "tool": tool,
                "risk": if source == "gizzi-permission" { "normal" } else { "normal" },
                "state": "pending",
                "created_at": created_at,
                "expires_at": Value::Null,
                "job_id": job_id,
                "resolve": { "method": "POST", "path": "/api/v1/cowork/approvals", "body": { "actionId": id, "decision": "approve" } },
            }),
            requester: user_id,
            approver: effective_approver,
            bot: bot_id,
        });
    }
    Ok(out)
}

fn factory_asks(conn: &Connection, scope: &[String]) -> rusqlite::Result<Vec<AskRec>> {
    let mut out = vec![];
    for owner in scope {
        for a in crate::factory_approvals::list(conn, owner, Some("pending"))? {
            out.push(AskRec {
                ask: json!({
                    "id": format!("factory:{}", a.id),
                    "store": "factory",
                    "title": a.title,
                    "summary": a.summary,
                    "actor": node_actor(a.bot_id.as_deref(), Some(&a.owner)),
                    "approver_user_id": a.owner,
                    "rule_id": Value::Null,
                    "app": Value::Null,
                    "tool": Value::Null,
                    "risk": a.risk,
                    "state": a.state,
                    "created_at": a.created_at,
                    "expires_at": a.expires_at,
                    "job_id": Value::Null,
                    "resolve": { "method": "POST", "path": format!("/api/factory/approvals/{}/resolve", a.id), "body": { "decision": "approve" } },
                }),
                requester: Some(a.owner.clone()),
                approver: Some(a.owner),
                bot: a.bot_id,
            });
        }
    }
    Ok(out)
}

/// Read the cloud routed list. `Err` = could not read (unpaired, offline, bad
/// status or shape): the caller records `partial` and serves local rows.
pub async fn fetch_cloud_asks(cloud: &dyn AccountCloud, me: &str) -> Result<Vec<AskRec2>, String> {
    let (status, body) = cloud.send("GET", CLOUD_ROUTED_APPROVALS_PATH, Vec::new()).await?;
    if !(200..300).contains(&status) {
        return Err(format!("cloud answered {status}"));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let data = v["data"].as_array().ok_or("cloud answer has no data[]")?;
    Ok(data
        .iter()
        .map(|d| {
            let approver = d["approver_user_id"].as_str().map(str::to_string).or_else(|| Some(me.to_string()));
            let actor = d["actor"].clone();
            let ask = json!({
                "id": d["id"], "store": d["store"], "title": d["title"], "summary": d["summary"],
                "actor": actor, "approver_user_id": approver, "rule_id": d["rule_id"],
                "app": Value::Null, "tool": d["tool"], "risk": "normal",
                "state": d["state"], "created_at": d["created_at"], "expires_at": d["expires_at"],
                "job_id": Value::Null, "resolve": d["resolve"],
            });
            AskRec2(AskRec { ask, requester: None, approver, bot: actor_bot(&d["actor"]) })
        })
        .collect())
}

/// Opaque wrapper so `AskRec` stays private while the fetch is public.
#[derive(Debug, Clone)]
pub struct AskRec2(AskRec);

fn actor_bot(actor: &Value) -> Option<String> {
    actor.as_str().and_then(|a| a.strip_prefix("b:")).map(str::to_string)
}

fn want_pending(ask: &Value) -> bool {
    ask["state"].as_str().map_or(true, |s| s == "pending")
}

/// The union of every store's pending asks for `scope` users, plus the cloud
/// rows already fetched. `scope` is the viewer, or every member for the map.
fn all_asks(conn: &Connection, scope: &[String], cloud: &Result<Vec<AskRec2>, String>) -> rusqlite::Result<Vec<AskRec>> {
    let mut out = cowork_asks(conn, scope)?;
    out.extend(factory_asks(conn, scope)?);
    if let Ok(rows) = cloud {
        out.extend(rows.iter().map(|r| r.0.clone()));
    }
    out.retain(|a| want_pending(&a.ask));
    out.sort_by(|a, b| b.ask["created_at"].as_str().cmp(&a.ask["created_at"].as_str()));
    Ok(out)
}

// ------------------------------------------------------------------ map

fn names_of(nodes: &[Value]) -> HashMap<String, String> {
    nodes.iter().filter_map(|n| Some((n["id"].as_str()?.to_string(), n["name"].as_str()?.to_string()))).collect()
}

fn app_info(conn: &Connection, kind: &str, app_ref: &str) -> Value {
    let name: Option<String> = if kind == "connector" {
        conn.query_row("SELECT name FROM mcp_connectors WHERE id = ?1", params![app_ref], |r| r.get(0)).optional().ok().flatten()
    } else {
        None
    };
    // TODO(lane 13/18): app icons (connector icon url, artifact app icon).
    json!({ "kind": kind, "ref": app_ref, "name": name.unwrap_or_else(|| app_ref.to_string()), "icon": Value::Null })
}

/// Sentence for a rule, rendered from the row.
pub fn rule_text(rule: &ApprovalRule, names: &HashMap<String, String>) -> String {
    let who = match rule.scope_kind {
        ScopeKind::Org => "anyone".to_string(),
        ScopeKind::Person => rule.scope_ref.as_ref().map(|r| names.get(&format!("p:{r}")).cloned().unwrap_or_else(|| r.clone())).unwrap_or_else(|| "a person".into()),
        ScopeKind::Bot => rule.scope_ref.as_ref().map(|r| names.get(r).cloned().unwrap_or_else(|| r.clone())).unwrap_or_else(|| "a bot".into()),
    };
    let approver = match rule.approver_kind {
        ApproverKind::Owner => "the owner".to_string(),
        ApproverKind::Manager => "the manager".to_string(),
        ApproverKind::User => rule
            .approver_user_id
            .as_ref()
            .map(|u| names.get(&format!("p:{u}")).cloned().unwrap_or_else(|| u.clone()))
            .unwrap_or_else(|| "a named person".into()),
    };
    let tool = if rule.tool == "*" { "any tool".to_string() } else { rule.tool.clone() };
    let app = rule.app_ref.as_ref().map(|r| format!(" in {r}")).unwrap_or_default();
    let shown = |v: &Option<Value>| match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(", "),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let unit = rule.unit_label.as_ref().map(|u| format!(" {u}")).unwrap_or_default();
    let field = rule.field.clone().unwrap_or_else(|| "the value".into());
    let cond = match rule.op {
        RuleOp::Always => String::new(),
        RuleOp::Gt => format!(" when {field} is over {}{unit}", shown(&rule.value)),
        RuleOp::Gte => format!(" when {field} is {}{unit} or more", shown(&rule.value)),
        RuleOp::Eq => format!(" when {field} is {}{unit}", shown(&rule.value)),
        RuleOp::In => format!(" when {field} is one of {}", shown(&rule.value)),
    };
    format!("{who} needs {approver} to approve {tool}{app}{cond}.")
}

/// Build the `/team/map` body from rows in hand. `cloud` is the cloud routed
/// read (`Err` fills `partial`).
pub fn map_json(conn: &Connection, v: &Viewer, cloud: &Result<Vec<AskRec2>, String>) -> rusqlite::Result<Value> {
    let mut partial: Vec<&str> = vec![];
    if cloud.is_err() {
        partial.push(PARTIAL_PLATFORM_APPROVALS);
    }
    partial.push(PARTIAL_APP_TIERS);

    // People. Solo account: org null, the viewer is the top node.
    let mut people: Vec<(String, String, Option<String>, Option<String>, Option<String>)> = vec![]; // id, name, avatar, role, reports_to
    let org = match v.org_id.as_deref() {
        Some(org_id) => {
            for m in team_store::list_members(conn, org_id)? {
                let name = m.name.clone().or(m.email.clone()).unwrap_or_else(|| m.user_id.clone());
                people.push((m.user_id, name, m.avatar_url, Some(m.role), m.reports_to_user_id));
            }
            let org_name: Option<String> =
                conn.query_row("SELECT name FROM organizations WHERE id = ?1", params![org_id], |r| r.get(0)).optional()?;
            Some(json!({ "id": org_id, "name": org_name.unwrap_or_default() }))
        }
        None => None,
    };
    if !people.iter().any(|p| p.0 == v.user_id) {
        people.push((v.user_id.clone(), v.name.clone().unwrap_or_else(|| "Me".into()), v.avatar.clone(), None, None));
    }
    let member_ids: Vec<String> = people.iter().map(|p| p.0.clone()).collect();

    let mut nodes: Vec<Value> = vec![];
    let mut edges: Vec<Value> = vec![];
    for (id, name, avatar, role, reports_to) in &people {
        nodes.push(json!({
            "id": format!("p:{id}"), "kind": "person", "name": name, "avatar": avatar, "role": role,
            "is_main": Value::Null, "computer": Value::Null,
            // No presence source in the runtime yet: me idle, others offline.
            "state": if *id == v.user_id { "idle" } else { "offline" },
            "counts": { "asks": 0, "jobs": 0 },
        }));
        if let Some(mgr) = reports_to {
            if member_ids.contains(mgr) {
                edges.push(json!({ "from": format!("p:{id}"), "to": format!("p:{mgr}"), "kind": "reports_to" }));
            }
        }
    }

    // Vendor (Connected) bots first so their agent rows are not shown twice.
    let mut vendors: Vec<(String, String, Option<String>)> = vec![];
    for owner in &member_ids {
        let mut stmt = conn.prepare("SELECT vendor_bot_id, owner, directing_bot_id FROM vendor_bot_connectors WHERE owner = ?1 ORDER BY vendor_bot_id")?;
        let rows = stmt.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        for r in rows {
            vendors.push(r?);
        }
    }
    let vendor_ids: Vec<String> = vendors.iter().map(|x| x.0.clone()).collect();

    let mut bot_status: HashMap<String, String> = HashMap::new();
    for owner in &member_ids {
        for b in crate::floor_routes::bot_states(conn, owner)? {
            if let (Some(id), Some(st)) = (b["id"].as_str(), b["status"].as_str()) {
                bot_status.insert(id.to_string(), st.to_string());
            }
        }
    }

    let mut bot_nodes: Vec<(String, BotKind)> = vec![];
    for owner in &member_ids {
        let mut stmt = conn.prepare(
            "SELECT id, name, avatar, parent_agent_id, COALESCE(config, '{}') FROM agents
             WHERE user_id = ?1 AND is_bot = 1 AND archived_at IS NULL ORDER BY name",
        )?;
        let rows = stmt.query_map(params![owner], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?))
        })?;
        for r in rows {
            let (id, name, avatar, parent, config) = r?;
            if vendor_ids.contains(&id) {
                continue;
            }
            let cfg: Value = serde_json::from_str(&config).unwrap_or(Value::Null);
            nodes.push(json!({
                "id": format!("b:{id}"), "kind": "bot", "name": name, "avatar": avatar, "role": Value::Null,
                "is_main": crate::floor_routes::config_is_main_bot(&cfg), "computer": Value::Null,
                "state": "idle", "counts": { "asks": 0, "jobs": 0 },
            }));
            edges.push(json!({ "from": format!("p:{owner}"), "to": format!("b:{id}"), "kind": "owns" }));
            if let Some(p) = parent.filter(|p| !p.is_empty()) {
                edges.push(json!({ "from": format!("b:{p}"), "to": format!("b:{id}"), "kind": "parent" }));
            }
            bot_nodes.push((id, BotKind::Agent));
        }
    }
    for (vid, owner, directing) in &vendors {
        let name: Option<String> = conn.query_row("SELECT name FROM agents WHERE id = ?1", params![vid], |r| r.get(0)).optional()?;
        nodes.push(json!({
            "id": format!("v:{vid}"), "kind": "bot", "name": name.unwrap_or_else(|| vid.clone()), "avatar": Value::Null, "role": Value::Null,
            "is_main": false, "computer": Value::Null,
            "state": "offline", "counts": { "asks": 0, "jobs": 0 },
        }));
        edges.push(json!({ "from": format!("p:{owner}"), "to": format!("v:{vid}"), "kind": "owns" }));
        if let Some(d) = directing.as_ref().filter(|d| !d.is_empty()) {
            edges.push(json!({ "from": format!("b:{d}"), "to": format!("v:{vid}"), "kind": "parent" }));
        }
        bot_nodes.push((vid.clone(), BotKind::Vendor));
    }

    // Asks: counts for everyone, details only where visible.
    let all = all_asks(conn, &member_ids, cloud)?;
    let mut ask_count: HashMap<String, i64> = HashMap::new();
    for a in &all {
        if let Some(b) = &a.bot {
            *ask_count.entry(format!("b:{b}")).or_default() += 1;
        }
        if let Some(u) = &a.approver {
            *ask_count.entry(format!("p:{u}")).or_default() += 1;
        }
    }
    for n in nodes.iter_mut() {
        let id = n["id"].as_str().unwrap_or("").to_string();
        let count = ask_count.get(&id).copied().unwrap_or(0);
        n["counts"]["asks"] = json!(count);
        if n["kind"] == "bot" {
            let bare = id.split_once(':').map(|x| x.1).unwrap_or("");
            if id.starts_with('b') && ask_count.contains_key(&id) {
                n["state"] = json!("waiting_approval");
            } else if let Some(st) = bot_status.get(bare) {
                n["state"] = json!(match st.as_str() {
                    "waiting" => "needs_input",
                    "working" => "working",
                    _ => "idle",
                });
            }
        } else if count > 0 && id == format!("p:{}", v.user_id) {
            n["state"] = json!("waiting_approval");
        }
    }
    let asks: Vec<Value> = all.iter().filter(|a| a.visible_to(v)).map(|a| a.ask.clone()).collect();

    // Grants.
    let mut grants: Vec<Value> = vec![];
    for (id, kind) in &bot_nodes {
        for g in team_store::list_grants_for_bot(conn, *kind, id)? {
            let ceiling = if g.app_kind == AppKind::Vendor { GrantLevel::App } else { GrantLevel::Full };
            grants.push(json!({
                "bot": kind.node_ref(id),
                "app": app_info(conn, g.app_kind.as_str(), &g.app_ref),
                "level": g.level.as_str(),
                "ceiling": ceiling.as_str(),
            }));
        }
    }

    // Rules.
    let names = names_of(&nodes);
    let rules: Vec<Value> = team_store::list_rules(conn, v.org_id.as_deref(), &v.user_id)?
        .iter()
        .map(|r| {
            json!({
                "id": r.id, "text": rule_text(r, &names), "enabled": r.enabled,
                "scope": { "kind": r.scope_kind.as_str(), "ref": r.scope_ref },
                "approver": { "kind": r.approver_kind.as_str(), "user_id": r.approver_user_id },
            })
        })
        .collect();

    Ok(json!({
        "org": org,
        "me": format!("p:{}", v.user_id),
        "nodes": nodes,
        "edges": edges,
        "grants": grants,
        "rules": rules,
        "asks": asks,
        // TODO(lane 15): read `bar_jobs` (V295) here; shape
        // {job_id, actor, waiting_on, app, tool, title, state}. Empty until then.
        "jobs": Vec::<Value>::new(),
        "partial": partial,
    }))
}

async fn map_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let cloud = fetch_cloud_asks(&SignedAccountCloud, &user.user_id).await;
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let v = resolve_viewer(&conn, &user).map_err(|e| e.to_string())?;
        map_json(&conn, &v, &cloud).map_err(|e| e.to_string())
    })
    .await;
    match res {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(e)) => {
            tracing::warn!("team map: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "team_map_failed")
        }
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "team_map_failed"),
    }
}

// ------------------------------------------------------------------ approvals

#[derive(Deserialize)]
struct ApprovalsQuery {
    role: Option<String>,
    state: Option<String>,
}

/// `/team/approvals` body: `{ asks: [Ask], partial: [] }`.
pub fn approvals_json(conn: &Connection, v: &Viewer, role: &str, cloud: &Result<Vec<AskRec2>, String>) -> rusqlite::Result<Value> {
    let me = vec![v.user_id.clone()];
    let mut all = all_asks(conn, &me, cloud)?;
    // The cloud list only holds asks addressed to this owner (approver role).
    all.retain(|a| match role {
        "requester" => a.requester.as_deref() == Some(&v.user_id),
        _ => a.approver.as_deref() == Some(&v.user_id),
    });
    let partial: Vec<&str> = if cloud.is_err() { vec![PARTIAL_PLATFORM_APPROVALS] } else { vec![] };
    Ok(json!({ "asks": all.iter().map(|a| a.ask.clone()).collect::<Vec<_>>(), "partial": partial }))
}

async fn approvals_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<ApprovalsQuery>) -> Response {
    let role = match q.role.as_deref().unwrap_or("approver") {
        r @ ("approver" | "requester") => r.to_string(),
        _ => return err(StatusCode::BAD_REQUEST, "role must be approver or requester"),
    };
    if q.state.as_deref().is_some_and(|s| s != "pending") {
        return err(StatusCode::BAD_REQUEST, "only state=pending is available");
    }
    let cloud = if role == "approver" { fetch_cloud_asks(&SignedAccountCloud, &user.user_id).await } else { Ok(vec![]) };
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let v = resolve_viewer(&conn, &user).map_err(|e| e.to_string())?;
        approvals_json(&conn, &v, &role, &cloud).map_err(|e| e.to_string())
    })
    .await;
    match res {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "team_approvals_failed"),
    }
}

// ------------------------------------------------------------------ access

/// `/team/apps/:kind/:ref/access` body: `{ people: [{user, level}], bots: [{bot, owner, level}] }`.
pub fn access_json(conn: &Connection, v: &Viewer, kind: AppKind, app_ref: &str) -> rusqlite::Result<Value> {
    let member_ids: Vec<String> = match v.org_id.as_deref() {
        Some(o) => team_store::list_members(conn, o)?.into_iter().map(|m| m.user_id).collect(),
        None => vec![v.user_id.clone()],
    };
    let mut people: Vec<Value> = vec![];
    if kind == AppKind::Connector {
        let owner: Option<String> = conn.query_row("SELECT user_id FROM mcp_connectors WHERE id = ?1", params![app_ref], |r| r.get(0)).optional()?.flatten();
        if let Some(o) = owner.filter(|o| member_ids.contains(o)) {
            people.push(json!({ "user": format!("p:{o}"), "level": "full" }));
        }
        let mut stmt = conn.prepare("SELECT DISTINCT user_id FROM mcp_app_installs WHERE connector_id = ?1 ORDER BY user_id")?;
        let users: Vec<String> = stmt.query_map(params![app_ref], |r| r.get(0))?.collect::<Result<_, _>>()?;
        for u in users {
            if member_ids.contains(&u) && !people.iter().any(|p| p["user"] == format!("p:{u}")) {
                people.push(json!({ "user": format!("p:{u}"), "level": "app" }));
            }
        }
    }
    let mut bots: Vec<Value> = vec![];
    for g in team_store::list_grants_for_app(conn, kind, app_ref)? {
        let owner: Option<String> = match g.bot_kind {
            BotKind::Agent => conn.query_row("SELECT user_id FROM agents WHERE id = ?1", params![g.bot_id], |r| r.get(0)).optional()?,
            BotKind::Vendor => conn.query_row("SELECT owner FROM vendor_bot_connectors WHERE vendor_bot_id = ?1", params![g.bot_id], |r| r.get(0)).optional()?,
        };
        // Bots outside the viewer's org are not part of this map.
        if !owner.as_ref().is_some_and(|o| member_ids.contains(o)) {
            continue;
        }
        bots.push(json!({ "bot": g.bot_kind.node_ref(&g.bot_id), "owner": owner.map(|o| format!("p:{o}")), "level": g.level.as_str() }));
    }
    Ok(json!({ "people": people, "bots": bots }))
}

async fn access_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((kind, app_ref)): Path<(String, String)>) -> Response {
    let Some(kind) = AppKind::parse(&kind) else {
        return err(StatusCode::BAD_REQUEST, "kind must be connector, vendor or artifact");
    };
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let v = resolve_viewer(&conn, &user).map_err(|e| e.to_string())?;
        access_json(&conn, &v, kind, &app_ref).map_err(|e| e.to_string())
    })
    .await;
    match res {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "team_access_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use async_trait::async_trait;

    struct Off;
    #[async_trait]
    impl AccountCloud for Off {
        async fn send(&self, _m: &str, _p: &str, _b: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
            Err("this computer is not paired".into())
        }
    }

    fn seeded() -> (Connection, Viewer) {
        let conn = DbHandle::new_memory().expect("migrated db").connect().expect("connect");
        conn.execute("INSERT INTO organizations (id, name) VALUES ('org1', 'Acme')", []).unwrap();
        for u in ["owner1", "emp1"] {
            conn.execute("INSERT OR IGNORE INTO users (id, email, name) VALUES (?1, ?2, ?1)", params![u, format!("{u}@x.test")]).unwrap();
        }
        for (u, role, mgr) in [("owner1", "owner", None), ("emp1", "member", Some("owner1"))] {
            conn.execute(
                "INSERT INTO organization_members (id, organization_id, user_id, role, reports_to_user_id) VALUES (?1, 'org1', ?2, ?3, ?4)",
                params![format!("org1:{u}"), u, role, mgr],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot) VALUES ('bot1', 'emp1', 'Scout', 'm', 'p', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, parent_agent_id, model, provider, is_bot) VALUES ('bot2', 'emp1', 'Helper', 'bot1', 'm', 'p', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cowork_approvals (id, user_id, content, source, dismissed, approver_user_id, bot_id)
             VALUES ('ap1', 'emp1', '{\"tool\":\"send_email\",\"title\":\"Send the quote\"}', 'gizzi-permission', 0, 'owner1', 'bot1')",
            [],
        )
        .unwrap();
        let v = Viewer { user_id: "owner1".into(), org_id: Some("org1".into()), is_admin: true, name: None, avatar: None };
        (conn, v)
    }

    #[tokio::test]
    async fn team_map_returns_nodes_edges_and_asks_for_a_seeded_org() {
        let (conn, v) = seeded();
        let cloud = fetch_cloud_asks(&Off, "owner1").await;
        let map = map_json(&conn, &v, &cloud).unwrap();
        let ids: Vec<&str> = map["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
        for want in ["p:owner1", "p:emp1", "b:bot1", "b:bot2"] {
            assert!(ids.contains(&want), "missing node {want}: {ids:?}");
        }
        let edges = map["edges"].as_array().unwrap();
        let has = |f: &str, t: &str, k: &str| edges.iter().any(|e| e["from"] == f && e["to"] == t && e["kind"] == k);
        assert!(has("p:emp1", "p:owner1", "reports_to"));
        assert!(has("p:emp1", "b:bot1", "owns"));
        assert!(has("b:bot1", "b:bot2", "parent"));
        let asks = map["asks"].as_array().unwrap();
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0]["id"], "cowork:ap1");
        assert_eq!(asks[0]["actor"], "b:bot1");
        let bot = map["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "b:bot1").unwrap();
        assert_eq!(bot["state"], "waiting_approval");
        assert_eq!(bot["counts"]["asks"], 1);
        assert_eq!(map["me"], "p:owner1");
        assert_eq!(map["org"]["name"], "Acme");
        // A plain member who is neither requester nor approver sees counts, not the ask.
        let other = Viewer { user_id: "emp2".into(), is_admin: false, ..v.clone() };
        let m2 = map_json(&conn, &other, &cloud).unwrap();
        assert!(m2["asks"].as_array().unwrap().is_empty());
        let bot2 = m2["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "b:bot1").unwrap();
        assert_eq!(bot2["counts"]["asks"], 1);
    }

    #[tokio::test]
    async fn team_map_partial_is_set_when_the_cloud_is_off() {
        let (conn, v) = seeded();
        let cloud = fetch_cloud_asks(&Off, "owner1").await;
        assert!(cloud.is_err());
        let map = map_json(&conn, &v, &cloud).unwrap();
        let partial: Vec<&str> = map["partial"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert!(partial.contains(&PARTIAL_PLATFORM_APPROVALS));
        // Local rows still come back.
        let a = approvals_json(&conn, &v, "approver", &cloud).unwrap();
        assert_eq!(a["asks"].as_array().unwrap().len(), 1);
        assert_eq!(a["asks"][0]["resolve"]["path"], "/api/v1/cowork/approvals");
        assert_eq!(a["partial"][0], PARTIAL_PLATFORM_APPROVALS);
    }

    #[test]
    fn team_apps_access_lists_a_granted_bot() {
        let (conn, v) = seeded();
        conn.execute("INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn1', 'owner1', 'Notion', 'notion', 'https://x')", []).unwrap();
        team_store::upsert_grant(&conn, BotKind::Agent, "bot1", AppKind::Connector, "conn1", GrantLevel::App, "owner1").unwrap();
        let body = access_json(&conn, &v, AppKind::Connector, "conn1").unwrap();
        assert_eq!(body["bots"][0]["bot"], "b:bot1");
        assert_eq!(body["bots"][0]["owner"], "p:emp1");
        assert_eq!(body["bots"][0]["level"], "app");
        assert_eq!(body["people"][0]["user"], "p:owner1");
        let map = map_json(&conn, &v, &Err("off".into())).unwrap();
        assert_eq!(map["grants"][0]["app"]["name"], "Notion");
    }
}
