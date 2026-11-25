//! Team map routes (Island bar + Team map program). Reads are lane 12, writes
//! lane 13.
//!
//! - `GET /team/map`: nodes (people, bots), edges, grants (with the real
//!   install ceiling), rules (with a rendered `text`), asks, jobs, and
//!   `partial[]` (cloud sources that failed).
//! - `GET /team/approvals?role=approver|requester&state=pending`: the one read
//!   model over `cowork_approvals`, `factory_approvals` and the cloud routed
//!   list. `resolve` on each Ask is that store's own resolve route.
//! - `GET /team/apps/:kind/:ref/access`: who can use one app.
//! - `PUT /team/people/:user_id {reports_to_user_id}`: owner/admin. 409 on a
//!   cycle, 422 for a non-member.
//! - `GET|PUT|DELETE /team/grants`: the bot's owner, or the org owner/admin.
//!   PUT checks the ceiling first (vendor app: `app`; artifact/vendor app: the
//!   owner's install tier from cloud `GET /api/v1/apps`; 503
//!   `tier_check_unavailable` when that cannot be read).
//! - `GET|POST /team/rules`, `PATCH|DELETE /team/rules/:rule_id`: writes are
//!   owner/admin; every member reads.
//!
//! Visibility: every member sees nodes, edges, levels and counts. Ask details
//! go only to the approver, the requester and an owner/admin.

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, put},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::account_sync::{AccountCloud, SignedAccountCloud};
use crate::auth::AuthUser;
use crate::bar_routes::{self, JobState};
use crate::team_store::{
    self, AppKind, ApprovalRule, ApproverKind, BotAppGrant, BotKind, GrantLevel, ReportsToOutcome, RuleInput, RuleOp, ScopeKind,
};
use crate::AppState;

/// Cloud routed approvals (`platform:` + `meeting:` rows addressed to this
/// runtime's owner). Lane 14b (cloud PR #2983) fixed the path; the response is
/// `{object:"list", data:[Ask-like rows]}`. Signed like `account_sync`.
pub const CLOUD_ROUTED_APPROVALS_PATH: &str = "/api/v1/approvals/routed?limit=100";

/// `partial[]` entry when the cloud routed list could not be read.
pub const PARTIAL_PLATFORM_APPROVALS: &str = "platform_approvals";
/// `partial[]` entry: the owner's install tiers (cloud `GET /api/v1/apps`)
/// could not be read, so an artifact/vendor grant's `ceiling` is the static
/// one (vendor: app, else full).
pub const PARTIAL_APP_TIERS: &str = "app_tiers";
/// The cloud app shelf: `{ yours: [row], servers: [row] }`, a row being
/// `{ kind: artifact|vendor, ref, name, icon, provenance: you|org|shared|server,
/// tier, installed, pinned, link }` (cloud-api `routes/apps.rs`). Signed like
/// `account_sync`, so it answers for this computer's paired owner.
pub const CLOUD_APPS_PATH: &str = "/api/v1/apps";

pub fn team_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/team/map", get(map_route))
        .route("/team/approvals", get(approvals_route))
        .route("/team/apps/:kind/:app_ref/access", get(access_route))
        .route("/team/people/:user_id", put(put_person_route))
        .route("/team/grants", get(list_grants_route).put(put_grant_route).delete(delete_grant_route))
        .route("/team/rules", get(list_rules_route).post(create_rule_route))
        .route("/team/rules/:rule_id", patch(patch_rule_route).delete(delete_rule_route))
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

/// `Ask.app` from a routed ask's own `app` (`{kind, ref, name}`, written by
/// `team_rules::open_ask`); `null` when the ask names no app.
fn ask_app(app: &Value) -> Value {
    match (app["kind"].as_str().and_then(AppKind::parse), app["ref"].as_str()) {
        (Some(kind), Some(app_ref)) => json!({
            "kind": kind.as_str(),
            "ref": app_ref,
            "name": app["name"].as_str().unwrap_or(app_ref),
            "icon": app["icon"].as_str(),
        }),
        _ => Value::Null,
    }
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
                "app": ask_app(&body["app"]),
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
                "app": ask_app(&d["app"]), "tool": d["tool"], "risk": "normal",
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

/// `{kind, ref, name, icon}` for an app: a connector's name from
/// `mcp_connectors`, an artifact/vendor app's name and icon from the cloud
/// shelf when it was read. Falls back to the ref.
fn app_info(conn: &Connection, kind: AppKind, app_ref: &str, tiers: &TierRead) -> Value {
    let (name, icon): (Option<String>, Option<String>) = match kind {
        AppKind::Connector => (
            conn.query_row("SELECT name FROM mcp_connectors WHERE id = ?1", params![app_ref], |r| r.get(0)).optional().ok().flatten(),
            None,
        ),
        _ => tiers.app(kind, app_ref).map(|a| (a.name.clone(), a.icon.clone())).unwrap_or((None, None)),
    };
    json!({ "kind": kind.as_str(), "ref": app_ref, "name": name.unwrap_or_else(|| app_ref.to_string()), "icon": icon })
}

/// Key of an app's display name in the `names` map that `rule_text` reads.
fn app_name_key(kind: AppKind, app_ref: &str) -> String {
    format!("app:{}:{app_ref}", kind.as_str())
}

/// Sentence for a rule: `team_rules::render_rule` (the one renderer the bar
/// review card and the ask card use too), with the names in `names` (node ids
/// `p:<user>` / `b:<id>` / `v:<id>`, and `app_name_key` for apps).
pub fn rule_text(rule: &ApprovalRule, names: &HashMap<String, String>) -> String {
    let scope = match rule.scope_kind {
        ScopeKind::Org => None,
        ScopeKind::Person => rule.scope_ref.as_ref().and_then(|r| names.get(&format!("p:{r}"))),
        ScopeKind::Bot => rule.scope_ref.as_ref().and_then(|r| names.get(r)),
    };
    let app = match (rule.app_kind, rule.app_ref.as_deref()) {
        (Some(k), Some(r)) => names.get(&app_name_key(k, r)),
        _ => None,
    };
    let approver = match rule.approver_kind {
        ApproverKind::User => rule.approver_user_id.as_ref().and_then(|u| names.get(&format!("p:{u}"))),
        ApproverKind::Owner | ApproverKind::Manager => None,
    };
    crate::team_rules::render_rule(
        rule,
        &crate::team_rules::RuleNames { scope: scope.map(String::as_str), app: app.map(String::as_str), approver: approver.map(String::as_str) },
    )
}

/// The bot's model label for its node: the model of `botBrain.modelRef`, else the
/// `botBrain.runtime` / config `runtime` id, else the agent row's `model` column; `null` when none is stored.
pub(crate) fn brain_label(cfg: &Value, column_model: Option<&str>) -> Value {
    let brain = cfg.get("botBrain").unwrap_or(&Value::Null);
    if let Some((_, model)) = crate::bot_brain::brain_model_pair(brain) {
        return json!(model);
    }
    if let Some(m) = brain.get("modelRef").and_then(Value::as_str).filter(|m| !m.is_empty()) {
        return json!(m);
    }
    brain
        .get("runtime")
        .or_else(|| cfg.get("runtime"))
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
        .or(column_model.filter(|m| !m.is_empty()))
        .map_or(Value::Null, |r| json!(r))
}

/// Build the `/team/map` body from rows in hand. `cloud` is the cloud routed
/// read and `tiers` the cloud app shelf read (`Err` in either fills `partial`).
pub fn map_json(conn: &Connection, v: &Viewer, cloud: &Result<Vec<AskRec2>, String>, tiers: &TierRead) -> rusqlite::Result<Value> {
    let mut partial: Vec<&str> = vec![];
    if cloud.is_err() {
        partial.push(PARTIAL_PLATFORM_APPROVALS);
    }

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

    let mut bot_nodes: Vec<(String, BotKind, String)> = vec![]; // id, kind, owner
    for owner in &member_ids {
        let mut stmt = conn.prepare(
            "SELECT id, name, avatar, parent_agent_id, COALESCE(config, '{}'), model FROM agents
             WHERE user_id = ?1 AND is_bot = 1 AND archived_at IS NULL ORDER BY name",
        )?;
        let rows = stmt.query_map(params![owner], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?))
        })?;
        for r in rows {
            let (id, name, avatar, parent, config, model) = r?;
            if vendor_ids.contains(&id) {
                continue;
            }
            let cfg: Value = serde_json::from_str(&config).unwrap_or(Value::Null);
            nodes.push(json!({
                "id": format!("b:{id}"), "kind": "bot", "name": name, "avatar": avatar, "role": Value::Null,
                "is_main": crate::floor_routes::config_is_main_bot(&cfg), "computer": Value::Null,
                "brain": brain_label(&cfg, model.as_deref()),
                "state": "idle", "counts": { "asks": 0, "jobs": 0 },
            }));
            edges.push(json!({ "from": format!("p:{owner}"), "to": format!("b:{id}"), "kind": "owns" }));
            if let Some(p) = parent.filter(|p| !p.is_empty()) {
                edges.push(json!({ "from": format!("b:{p}"), "to": format!("b:{id}"), "kind": "parent" }));
            }
            bot_nodes.push((id, BotKind::Agent, owner.clone()));
        }
    }
    for (vid, owner, directing) in &vendors {
        let row: Option<(String, String, Option<String>)> = conn
            .query_row("SELECT name, COALESCE(config, '{}'), model FROM agents WHERE id = ?1", params![vid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let (name, vcfg, vmodel) = match row {
            Some((n, c, m)) => (Some(n), serde_json::from_str::<Value>(&c).unwrap_or(Value::Null), m),
            None => (None, Value::Null, None),
        };
        nodes.push(json!({
            "id": format!("v:{vid}"), "kind": "bot", "name": name.unwrap_or_else(|| vid.clone()), "avatar": Value::Null, "role": Value::Null,
            "is_main": false, "computer": Value::Null, "brain": brain_label(&vcfg, vmodel.as_deref()),
            "state": "offline", "counts": { "asks": 0, "jobs": 0 },
        }));
        edges.push(json!({ "from": format!("p:{owner}"), "to": format!("v:{vid}"), "kind": "owns" }));
        if let Some(d) = directing.as_ref().filter(|d| !d.is_empty()) {
            edges.push(json!({ "from": format!("b:{d}"), "to": format!("v:{vid}"), "kind": "parent" }));
        }
        bot_nodes.push((vid.clone(), BotKind::Vendor, owner.clone()));
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

    // Bar jobs (V295): every member's active jobs. Counts on the actor's node and,
    // while waiting, the approver's; details only where an ask's would be visible.
    let mut jobs: Vec<Value> = vec![];
    let mut job_count: HashMap<String, i64> = HashMap::new();
    let mut job_state: HashMap<String, &'static str> = HashMap::new();
    for member in &member_ids {
        for j in bar_routes::list_jobs(conn, member, &JobState::ACTIVE)? {
            let actor = match j.bot_id.as_deref() {
                Some(b) if vendor_ids.iter().any(|x| x == b) => format!("v:{b}"),
                Some(b) => format!("b:{b}"),
                None => format!("p:{}", j.user_id),
            };
            let waiting_on = if j.state == JobState::WaitingApproval { j.approver_user_id.as_ref().map(|u| format!("p:{u}")) } else { None };
            *job_count.entry(actor.clone()).or_default() += 1;
            if let Some(w) = waiting_on.as_ref().filter(|w| **w != actor) {
                *job_count.entry(w.clone()).or_default() += 1;
            }
            job_state.entry(actor.clone()).or_insert(match j.state {
                JobState::WaitingApproval => "waiting_approval",
                JobState::InputRequired => "needs_input",
                _ => "working",
            });
            let visible = v.is_admin || j.user_id == v.user_id || j.approver_user_id.as_deref() == Some(v.user_id.as_str());
            if visible {
                jobs.push(json!({
                    "job_id": j.id,
                    "actor": actor,
                    "waiting_on": waiting_on,
                    "app": app_info(conn, j.app_kind, &j.app_ref, tiers),
                    "tool": j.tool,
                    "title": crate::mcp_server_routes::tool_label(&j.tool),
                    "state": j.state.as_str(),
                }));
            }
        }
    }
    for n in nodes.iter_mut() {
        let id = n["id"].as_str().unwrap_or("").to_string();
        n["counts"]["jobs"] = json!(job_count.get(&id).copied().unwrap_or(0));
        // A running bar job shows on an otherwise idle node.
        if let (Some(st), Some("idle")) = (job_state.get(&id), n["state"].as_str()) {
            n["state"] = json!(st);
        }
    }

    // Grants, with the real ceiling where the owner's install tier was read.
    let mut grants: Vec<Value> = vec![];
    let mut tiers_unknown = tiers.shelf.is_err();
    for (id, kind, owner) in &bot_nodes {
        for g in team_store::list_grants_for_bot(conn, *kind, id)? {
            let (grant, unknown) = grant_json(conn, &g, owner, tiers)?;
            tiers_unknown |= unknown;
            grants.push(grant);
        }
    }
    if tiers_unknown {
        partial.push(PARTIAL_APP_TIERS);
    }

    // Rules, with app display names where the connector registry or the shelf has them.
    let mut names = names_of(&nodes);
    let rule_rows = team_store::list_rules(conn, v.org_id.as_deref(), &v.user_id)?;
    for r in &rule_rows {
        if let (Some(k), Some(app_ref)) = (r.app_kind, r.app_ref.as_deref()) {
            if let Some(name) = app_info(conn, k, app_ref, tiers)["name"].as_str() {
                names.insert(app_name_key(k, app_ref), name.to_string());
            }
        }
    }
    let rules: Vec<Value> = rule_rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id, "text": rule_text(r, &names), "enabled": r.enabled,
                "scope": { "kind": r.scope_kind.as_str(), "ref": r.scope_ref },
                "approver": { "kind": r.approver_kind.as_str(), "user_id": r.approver_user_id },
                "locked": false,
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
        "jobs": jobs,
        "partial": partial,
    }))
}

async fn map_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let (cloud, tiers) =
        tokio::join!(fetch_cloud_asks(&SignedAccountCloud, &user.user_id), TierRead::read(&SignedAccountCloud, paired_owner()));
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let v = resolve_viewer(&conn, &user).map_err(|e| e.to_string())?;
        map_json(&conn, &v, &cloud, &tiers).map_err(|e| e.to_string())
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

// ------------------------------------------------------------------ install tiers (lane 13)

/// One app on the owner's cloud shelf (`GET /api/v1/apps`).
#[derive(Debug, Clone)]
pub struct ShelfApp {
    pub kind: AppKind,
    pub app_ref: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    /// you | org | shared | server
    pub provenance: String,
    /// The tier the app asks for (`app_permission_tier`, or the vendor app's tier).
    pub tier: Option<GrantLevel>,
    pub installed: bool,
    /// The install's own `app_installs.granted_tier`, when the cloud sends it.
    pub granted_tier: Option<GrantLevel>,
}

/// Read the shelf. `Err` = could not read (unpaired, offline, bad status or shape).
pub async fn fetch_shelf(cloud: &dyn AccountCloud) -> Result<Vec<ShelfApp>, String> {
    let (status, body) = cloud.send("GET", CLOUD_APPS_PATH, Vec::new()).await?;
    if !(200..300).contains(&status) {
        return Err(format!("cloud answered {status}"));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for key in ["yours", "servers"] {
        let rows = v[key].as_array().ok_or_else(|| format!("cloud answer has no {key}[]"))?;
        for r in rows {
            let (Some(kind), Some(app_ref)) = (r["kind"].as_str().and_then(AppKind::parse), r["ref"].as_str()) else {
                continue;
            };
            out.push(ShelfApp {
                kind,
                app_ref: app_ref.to_string(),
                name: r["name"].as_str().map(str::to_string),
                icon: r["icon"].as_str().map(str::to_string),
                provenance: r["provenance"].as_str().unwrap_or("").to_string(),
                tier: r["tier"].as_str().and_then(GrantLevel::parse),
                installed: r["installed"].as_bool().unwrap_or(false),
                granted_tier: r["granted_tier"].as_str().and_then(GrantLevel::parse),
            });
        }
    }
    Ok(out)
}

/// The shelf read plus whose shelf it is (the signed cloud client always
/// answers for this computer's paired owner).
#[derive(Debug, Clone)]
pub struct TierRead {
    pub paired_owner: Option<String>,
    pub shelf: Result<Vec<ShelfApp>, String>,
}

impl TierRead {
    pub async fn read(cloud: &dyn AccountCloud, paired_owner: Option<String>) -> Self {
        let shelf = match paired_owner {
            Some(_) => fetch_shelf(cloud).await,
            None => Err("this computer is not paired".into()),
        };
        TierRead { paired_owner, shelf }
    }

    /// No cloud read was needed (connector-only writes).
    pub fn skipped() -> Self {
        TierRead { paired_owner: None, shelf: Err("not read".into()) }
    }

    fn app(&self, kind: AppKind, app_ref: &str) -> Option<&ShelfApp> {
        self.shelf.as_ref().ok()?.iter().find(|a| a.kind == kind && a.app_ref == app_ref)
    }
}

fn paired_owner() -> Option<String> {
    crate::relay_auth::process_secret().paired_owner()
}

/// The most a bot may get on an app.
#[derive(Debug, Clone, PartialEq)]
pub enum Ceiling {
    Max(GrantLevel),
    /// The bot's owner has no install of the app (or the app is unknown).
    NotInstalled,
    /// The owner's install tier could not be read.
    Unknown(String),
}

/// Static cap by app kind: an app from another server never runs above `app`.
fn kind_cap(kind: AppKind) -> GrantLevel {
    if kind == AppKind::Vendor {
        GrantLevel::App
    } else {
        GrantLevel::Full
    }
}

/// The owner's install level for a shelf row. `full` only for an app the
/// owner or their org made.
pub(crate) fn install_ceiling(a: &ShelfApp) -> Option<GrantLevel> {
    if !a.installed {
        return None;
    }
    let provenance_cap = if matches!(a.provenance.as_str(), "you" | "org") { GrantLevel::Full } else { GrantLevel::App };
    // Until the shelf sends `granted_tier`, the app's own tier is the closest
    // bound; a link share installs at browser, so `shared` stays at browser.
    let granted = match a.granted_tier {
        Some(g) => g,
        None if a.provenance == "shared" => GrantLevel::Browser,
        None => a.tier.unwrap_or(GrantLevel::Browser),
    };
    Some(granted.min(provenance_cap).min(kind_cap(a.kind)))
}

/// Ceiling for a bot of `owner` on an app. Connectors are local: `full` for
/// the owner's own connector, `app` for someone else's.
pub fn grant_ceiling(conn: &Connection, owner: &str, kind: AppKind, app_ref: &str, tiers: &TierRead) -> rusqlite::Result<Ceiling> {
    if kind == AppKind::Connector {
        let row: Option<Option<String>> =
            conn.query_row("SELECT user_id FROM mcp_connectors WHERE id = ?1", params![app_ref], |r| r.get(0)).optional()?;
        return Ok(match row {
            None => Ceiling::NotInstalled,
            Some(None) => Ceiling::Max(GrantLevel::Full),
            Some(Some(u)) if u == owner => Ceiling::Max(GrantLevel::Full),
            Some(Some(_)) => Ceiling::Max(GrantLevel::App),
        });
    }
    if let Err(e) = &tiers.shelf {
        return Ok(Ceiling::Unknown(e.clone()));
    }
    if tiers.paired_owner.as_deref() != Some(owner) {
        return Ok(Ceiling::Unknown("this computer reads install tiers for its own owner only".into()));
    }
    Ok(match tiers.app(kind, app_ref).and_then(install_ceiling) {
        Some(level) => Ceiling::Max(level),
        None => Ceiling::NotInstalled,
    })
}

/// A grant row as the map and `/team/grants` show it. The bool is true when
/// the ceiling could not be read (the static one is shown).
fn grant_json(conn: &Connection, g: &BotAppGrant, owner: &str, tiers: &TierRead) -> rusqlite::Result<(Value, bool)> {
    let (ceiling, unknown) = match grant_ceiling(conn, owner, g.app_kind, &g.app_ref, tiers)? {
        Ceiling::Max(level) => (json!(level.as_str()), false),
        Ceiling::NotInstalled => (Value::Null, false),
        Ceiling::Unknown(_) => (json!(kind_cap(g.app_kind).as_str()), true),
    };
    Ok((
        json!({
            "id": g.id,
            "bot": g.bot_kind.node_ref(&g.bot_id),
            "app": app_info(conn, g.app_kind, &g.app_ref, tiers),
            "level": g.level.as_str(),
            "ceiling": ceiling,
            "granted_by": g.granted_by,
            "created_at": g.created_at,
        }),
        unknown,
    ))
}

// ------------------------------------------------------------------ writes (lane 13)

/// A refused write: status plus `{error, message, ...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub status: StatusCode,
    pub body: Value,
}

impl Fail {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Fail { status, body: json!({ "error": code, "message": message.into() }) }
    }
    fn with(mut self, key: &str, value: Value) -> Self {
        self.body[key] = value;
        self
    }
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        tracing::warn!("team write: {e}");
        Fail::new(StatusCode::INTERNAL_SERVER_ERROR, "team_write_failed", "The team data could not be read or saved.")
    }
}

pub type WriteResult = Result<Value, Fail>;

fn owner_or_admin_required() -> Fail {
    Fail::new(StatusCode::FORBIDDEN, "owner_or_admin_required", "Only the org owner or an admin can change this.")
}

fn unprocessable(code: &str, message: impl Into<String>) -> Fail {
    Fail::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
}

fn bot_not_found() -> Fail {
    Fail::new(StatusCode::NOT_FOUND, "bot_not_found", "That bot is not part of this team.")
}

/// Members of the viewer's org (a solo account: the viewer), viewer included.
fn member_ids(conn: &Connection, v: &Viewer) -> rusqlite::Result<Vec<String>> {
    let mut ids: Vec<String> = match v.org_id.as_deref() {
        Some(o) => team_store::list_members(conn, o)?.into_iter().map(|m| m.user_id).collect(),
        None => vec![],
    };
    if !ids.contains(&v.user_id) {
        ids.push(v.user_id.clone());
    }
    Ok(ids)
}

fn bot_owner(conn: &Connection, kind: BotKind, bot_id: &str) -> rusqlite::Result<Option<String>> {
    let sql = match kind {
        BotKind::Agent => "SELECT user_id FROM agents WHERE id = ?1",
        BotKind::Vendor => "SELECT owner FROM vendor_bot_connectors WHERE vendor_bot_id = ?1",
    };
    Ok(conn.query_row(sql, params![bot_id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
}

/// `b:<id>` / `v:<id>` → the bot.
fn parse_bot_ref(r: &str) -> Option<(BotKind, &str)> {
    match r.split_once(':')? {
        ("b", id) if !id.is_empty() => Some((BotKind::Agent, id)),
        ("v", id) if !id.is_empty() => Some((BotKind::Vendor, id)),
        _ => None,
    }
}

/// Every bot whose owner is a member: `(kind, id, owner)`.
fn org_bots(conn: &Connection, members: &[String]) -> rusqlite::Result<Vec<(BotKind, String, String)>> {
    let mut out = vec![];
    for owner in members {
        let mut stmt = conn.prepare("SELECT vendor_bot_id FROM vendor_bot_connectors WHERE owner = ?1 ORDER BY vendor_bot_id")?;
        let vendors: Vec<String> = stmt.query_map(params![owner], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut stmt =
            conn.prepare("SELECT id FROM agents WHERE user_id = ?1 AND is_bot = 1 AND archived_at IS NULL ORDER BY name, id")?;
        let agents: Vec<String> = stmt.query_map(params![owner], |r| r.get(0))?.collect::<Result<_, _>>()?;
        for id in agents.into_iter().filter(|a| !vendors.contains(a)) {
            out.push((BotKind::Agent, id, owner.clone()));
        }
        for id in vendors {
            out.push((BotKind::Vendor, id, owner.clone()));
        }
    }
    Ok(out)
}

/// Run a write against the database as the resolved viewer.
async fn run_team<F>(state: Arc<AppState>, user: AuthUser, f: F) -> Response
where
    F: FnOnce(&Connection, &Viewer) -> WriteResult + Send + 'static,
{
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> WriteResult {
        let conn = db.connect()?;
        let v = resolve_viewer(&conn, &user)?;
        f(&conn, &v)
    })
    .await;
    match res {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(fail)) => fail.into_response(),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "team_write_failed"),
    }
}

// ------------------------------------------------------------------ people

/// `PUT /team/people/:user_id {reports_to_user_id}` (null = top). Owner/admin.
/// `200 {user_id, reports_to_user_id}`.
pub fn put_person(conn: &Connection, v: &Viewer, user_id: &str, body: &Value) -> WriteResult {
    if !v.is_admin {
        return Err(owner_or_admin_required());
    }
    let Some(org) = v.org_id.as_deref() else {
        return Err(unprocessable("no_org", "Reports-to is for people in an organization."));
    };
    let manager = match body.get("reports_to_user_id") {
        None => return Err(unprocessable("invalid_body", "Send reports_to_user_id (a member's user id, or null for the top).")),
        Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(_) => return Err(unprocessable("invalid_body", "reports_to_user_id must be a user id or null.")),
    };
    match team_store::set_reports_to(conn, org, user_id, manager.as_deref())? {
        ReportsToOutcome::Set => Ok(json!({ "user_id": user_id, "reports_to_user_id": manager })),
        ReportsToOutcome::NotMember => Err(unprocessable("not_member", "That person is not a member of this organization.")),
        ReportsToOutcome::ManagerNotMember => {
            Err(unprocessable("manager_not_member", "The manager is not a member of this organization."))
        }
        ReportsToOutcome::SelfReport => Err(unprocessable("self_report", "A person cannot report to themself.")),
        ReportsToOutcome::Cycle => Err(Fail::new(
            StatusCode::CONFLICT,
            "reports_to_cycle",
            "That manager already reports to this person, so this would make a loop.",
        )),
    }
}

async fn put_person_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    run_team(state, user, move |conn, v| put_person(conn, v, &user_id, &body)).await
}

// ------------------------------------------------------------------ grants

struct GrantTarget {
    bot_kind: BotKind,
    bot_id: String,
    app_kind: AppKind,
    app_ref: String,
    owner: String,
}

/// Parse `{bot_kind, bot_id, app_kind, app_ref}` and check the viewer may
/// manage that bot: its owner, or the org owner/admin (E3).
fn grant_target(conn: &Connection, v: &Viewer, body: &Value) -> Result<GrantTarget, Fail> {
    let text = |k: &str| body[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let bot_kind = body["bot_kind"]
        .as_str()
        .and_then(BotKind::parse)
        .ok_or_else(|| unprocessable("invalid_body", "bot_kind must be agent or vendor."))?;
    let bot_id = text("bot_id").ok_or_else(|| unprocessable("invalid_body", "Send bot_id."))?;
    let app_kind = body["app_kind"]
        .as_str()
        .and_then(AppKind::parse)
        .ok_or_else(|| unprocessable("invalid_body", "app_kind must be connector, vendor or artifact."))?;
    let app_ref = text("app_ref").ok_or_else(|| unprocessable("invalid_body", "Send app_ref."))?;
    let members = member_ids(conn, v)?;
    let owner = bot_owner(conn, bot_kind, &bot_id)?.filter(|o| members.contains(o)).ok_or_else(bot_not_found)?;
    let manages = owner == v.user_id || (v.is_admin && v.org_id.is_some());
    if !manages {
        return Err(Fail::new(
            StatusCode::FORBIDDEN,
            "bot_owner_or_admin_required",
            "Only the bot's owner, the org owner or an admin can change its levels.",
        ));
    }
    Ok(GrantTarget { bot_kind, bot_id, app_kind, app_ref, owner })
}

/// `PUT /team/grants {bot_kind, bot_id, app_kind, app_ref, level}` → `200 {grant}`.
/// Ceiling first: 422 `above_ceiling {ceiling}`, 422 `app_not_installed`,
/// 503 `tier_check_unavailable`.
pub fn put_grant(conn: &Connection, v: &Viewer, body: &Value, tiers: &TierRead) -> WriteResult {
    let t = grant_target(conn, v, body)?;
    let level = body["level"]
        .as_str()
        .and_then(GrantLevel::parse)
        .ok_or_else(|| unprocessable("invalid_body", "level must be browser, app or full."))?;
    let above = |ceiling: GrantLevel| {
        unprocessable("above_ceiling", format!("This app can be given at most {} here.", ceiling.as_str()))
            .with("ceiling", json!(ceiling.as_str()))
    };
    // Static cap needs no cloud read.
    if level > kind_cap(t.app_kind) {
        return Err(above(kind_cap(t.app_kind)));
    }
    match grant_ceiling(conn, &t.owner, t.app_kind, &t.app_ref, tiers)? {
        Ceiling::Unknown(detail) => {
            return Err(Fail::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "tier_check_unavailable",
                "The owner's install level for this app could not be checked. Try again when this computer is online.",
            )
            .with("detail", json!(detail)))
        }
        Ceiling::NotInstalled => {
            return Err(unprocessable("app_not_installed", "The bot's owner has not installed this app."))
        }
        Ceiling::Max(ceiling) if level > ceiling => return Err(above(ceiling)),
        Ceiling::Max(_) => {}
    }
    let g = team_store::upsert_grant(conn, t.bot_kind, &t.bot_id, t.app_kind, &t.app_ref, level, &v.user_id)?;
    let (grant, _) = grant_json(conn, &g, &t.owner, tiers)?;
    Ok(json!({ "grant": grant }))
}

/// `DELETE /team/grants?bot_kind&bot_id&app_kind&app_ref` → `200 {revoked}`.
pub fn delete_grant(conn: &Connection, v: &Viewer, query: &Value) -> WriteResult {
    let t = grant_target(conn, v, query)?;
    let revoked = team_store::revoke_grant(conn, t.bot_kind, &t.bot_id, t.app_kind, &t.app_ref)?;
    Ok(json!({ "revoked": revoked }))
}

/// `GET /team/grants[?bot_kind&bot_id]` → `{grants: [grant], partial}`. Every
/// member reads levels (map visibility).
pub fn list_grants(conn: &Connection, v: &Viewer, query: &Value, tiers: &TierRead) -> WriteResult {
    let members = member_ids(conn, v)?;
    let bots = match (query["bot_kind"].as_str(), query["bot_id"].as_str()) {
        (None, None) => org_bots(conn, &members)?,
        (Some(k), Some(id)) => {
            let kind = BotKind::parse(k).ok_or_else(|| unprocessable("invalid_query", "bot_kind must be agent or vendor."))?;
            let owner = bot_owner(conn, kind, id)?.filter(|o| members.contains(o)).ok_or_else(bot_not_found)?;
            vec![(kind, id.to_string(), owner)]
        }
        _ => return Err(unprocessable("invalid_query", "Send bot_kind and bot_id together.")),
    };
    let mut grants = vec![];
    let mut unknown = tiers.shelf.is_err();
    for (kind, id, owner) in &bots {
        for g in team_store::list_grants_for_bot(conn, *kind, id)? {
            let (grant, u) = grant_json(conn, &g, owner, tiers)?;
            unknown |= u;
            grants.push(grant);
        }
    }
    let partial: Vec<&str> = if unknown { vec![PARTIAL_APP_TIERS] } else { vec![] };
    Ok(json!({ "grants": grants, "partial": partial }))
}

async fn list_grants_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let tiers = TierRead::read(&SignedAccountCloud, paired_owner()).await;
    let query = json!(q);
    run_team(state, user, move |conn, v| list_grants(conn, v, &query, &tiers)).await
}

async fn put_grant_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<Value>) -> Response {
    // Only artifact and vendor apps need the cloud install tier.
    let tiers = if body["app_kind"].as_str() == Some("connector") {
        TierRead::skipped()
    } else {
        TierRead::read(&SignedAccountCloud, paired_owner()).await
    };
    run_team(state, user, move |conn, v| put_grant(conn, v, &body, &tiers)).await
}

async fn delete_grant_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let query = json!(q);
    run_team(state, user, move |conn, v| delete_grant(conn, v, &query)).await
}

// ------------------------------------------------------------------ rules

/// Fields a rule body may set. `value_json` is accepted as another name for
/// `value` (a JSON value, not an encoded string).
const RULE_KEYS: [&str; 12] = [
    "scope_kind",
    "scope_ref",
    "app_kind",
    "app_ref",
    "tool",
    "field",
    "op",
    "value",
    "unit_label",
    "approver_kind",
    "approver_user_id",
    "enabled",
];

fn invalid_rule(message: impl Into<String>) -> Fail {
    unprocessable("invalid_rule", message)
}

/// The editable fields of a stored rule, for a PATCH to merge into.
fn rule_fields(r: &ApprovalRule) -> Value {
    json!({
        "scope_kind": r.scope_kind.as_str(), "scope_ref": r.scope_ref,
        "app_kind": r.app_kind.map(AppKind::as_str), "app_ref": r.app_ref,
        "tool": r.tool, "field": r.field, "op": r.op.as_str(), "value": r.value, "unit_label": r.unit_label,
        "approver_kind": r.approver_kind.as_str(), "approver_user_id": r.approver_user_id, "enabled": r.enabled,
    })
}

fn overlay_rule(base: &mut Value, body: &Value) -> Result<(), Fail> {
    let obj = body.as_object().ok_or_else(|| invalid_rule("Send the rule as a JSON object."))?;
    for (k, val) in obj {
        let key = if k == "value_json" { "value" } else { k.as_str() };
        if !RULE_KEYS.contains(&key) {
            return Err(invalid_rule(format!("{k} is not a rule field.")));
        }
        base[key] = val.clone();
    }
    Ok(())
}

fn opt_text(f: &Value, k: &str, max: usize) -> Result<Option<String>, Fail> {
    match &f[k] {
        Value::Null => Ok(None),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                Ok(None)
            } else if s.chars().count() > max {
                Err(invalid_rule(format!("{k} is longer than {max} characters.")))
            } else {
                Ok(Some(s.to_string()))
            }
        }
        _ => Err(invalid_rule(format!("{k} must be text."))),
    }
}

fn enum_field<T>(f: &Value, k: &str, parse: fn(&str) -> Option<T>, allowed: &str) -> Result<T, Fail> {
    f[k].as_str().and_then(parse).ok_or_else(|| invalid_rule(format!("{k} must be {allowed}.")))
}

/// One input field name: letters, digits, `_`, `.` (nested), `-`.
fn valid_field(f: &str) -> bool {
    f.len() <= 64 && f.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Validate a merged rule body into the row to store.
fn rule_input(conn: &Connection, v: &Viewer, f: &Value, org_id: Option<String>, owner_user_id: String) -> Result<RuleInput, Fail> {
    let members = member_ids(conn, v)?;

    let scope_kind = enum_field(f, "scope_kind", ScopeKind::parse, "org, person or bot")?;
    let scope_ref = opt_text(f, "scope_ref", 200)?;
    match scope_kind {
        ScopeKind::Org => {
            if scope_ref.is_some() {
                return Err(invalid_rule("scope_ref is only for a person or bot rule."));
            }
        }
        ScopeKind::Person => match &scope_ref {
            Some(r) if members.contains(r) => {}
            Some(_) => return Err(invalid_rule("scope_ref is not a member of this organization.")),
            None => return Err(invalid_rule("A person rule needs scope_ref (their user id).")),
        },
        ScopeKind::Bot => {
            let r = scope_ref.as_deref().ok_or_else(|| invalid_rule("A bot rule needs scope_ref (b:<id> or v:<id>)."))?;
            let (kind, id) = parse_bot_ref(r).ok_or_else(|| invalid_rule("scope_ref for a bot is b:<id> or v:<id>."))?;
            if !bot_owner(conn, kind, id)?.is_some_and(|o| members.contains(&o)) {
                return Err(invalid_rule("That bot is not part of this organization."));
            }
        }
    }

    let app_kind = match &f["app_kind"] {
        Value::Null => None,
        _ => Some(enum_field(f, "app_kind", AppKind::parse, "connector, vendor or artifact")?),
    };
    let app_ref = opt_text(f, "app_ref", 200)?;
    if app_kind.is_some() != app_ref.is_some() {
        return Err(invalid_rule("app_kind and app_ref go together."));
    }
    let tool = opt_text(f, "tool", 128)?.unwrap_or_else(|| "*".to_string());

    let op = enum_field(f, "op", RuleOp::parse, "always, gt, gte, eq or in")?;
    let field = opt_text(f, "field", 64)?;
    let value = match &f["value"] {
        Value::Null => None,
        x => Some(x.clone()),
    };
    let scalar = |x: &Value| x.is_string() || x.is_number() || x.is_boolean();
    match op {
        RuleOp::Always => {
            if field.is_some() || value.is_some() {
                return Err(invalid_rule("An always rule has no field or value."));
            }
        }
        _ => {
            let name = field.as_deref().ok_or_else(|| invalid_rule("This rule needs the input field it compares."))?;
            if !valid_field(name) {
                return Err(invalid_rule("field is letters, digits, _, . or - (64 at most)."));
            }
            let ok = match (op, &value) {
                (RuleOp::Gt | RuleOp::Gte, Some(x)) => x.is_number(),
                (RuleOp::Eq, Some(x)) => scalar(x),
                (RuleOp::In, Some(Value::Array(a))) => !a.is_empty() && a.len() <= 50 && a.iter().all(scalar),
                _ => false,
            };
            if !ok {
                return Err(invalid_rule(match op {
                    RuleOp::Gt | RuleOp::Gte => "value must be a number for over / at least.",
                    RuleOp::Eq => "value must be text, a number or true/false.",
                    _ => "value must be a list of 1 to 50 values.",
                }));
            }
        }
    }
    let unit_label = opt_text(f, "unit_label", 24)?;

    let approver_kind = enum_field(f, "approver_kind", ApproverKind::parse, "owner, manager or user")?;
    let approver_user_id = opt_text(f, "approver_user_id", 200)?;
    match (approver_kind, &approver_user_id) {
        (ApproverKind::User, Some(u)) if members.contains(u) => {}
        (ApproverKind::User, Some(_)) => return Err(invalid_rule("approver_user_id is not a member of this organization.")),
        (ApproverKind::User, None) => return Err(invalid_rule("A user approver needs approver_user_id.")),
        (_, Some(_)) => return Err(invalid_rule("approver_user_id is only for approver_kind user.")),
        (_, None) => {}
    }
    let enabled = match &f["enabled"] {
        Value::Null => true,
        Value::Bool(b) => *b,
        _ => return Err(invalid_rule("enabled must be true or false.")),
    };

    Ok(RuleInput {
        org_id,
        owner_user_id,
        scope_kind,
        scope_ref,
        app_kind,
        app_ref,
        tool,
        field,
        op,
        value,
        unit_label,
        approver_kind,
        approver_user_id,
        enabled,
    })
}

/// Names for rule sentences without the full map: members, scoped bots, and
/// connector apps from the connector registry.
fn rule_names(conn: &Connection, v: &Viewer, rules: &[ApprovalRule]) -> rusqlite::Result<HashMap<String, String>> {
    let mut names = HashMap::new();
    match v.org_id.as_deref() {
        Some(o) => {
            for m in team_store::list_members(conn, o)? {
                let name = m.name.or(m.email).unwrap_or_else(|| m.user_id.clone());
                names.insert(format!("p:{}", m.user_id), name);
            }
        }
        None => {
            names.insert(format!("p:{}", v.user_id), v.name.clone().unwrap_or_else(|| "you".into()));
        }
    }
    for r in rules {
        if let Some((kind, id)) = r.scope_ref.as_deref().filter(|_| r.scope_kind == ScopeKind::Bot).and_then(parse_bot_ref) {
            let name: Option<String> =
                conn.query_row("SELECT name FROM agents WHERE id = ?1", params![id], |row| row.get(0)).optional()?;
            if let Some(n) = name {
                names.insert(kind.node_ref(id), n);
            }
        }
        if let (Some(AppKind::Connector), Some(app_ref)) = (r.app_kind, r.app_ref.as_deref()) {
            let name: Option<String> =
                conn.query_row("SELECT name FROM mcp_connectors WHERE id = ?1", params![app_ref], |row| row.get(0)).optional()?;
            if let Some(n) = name {
                names.insert(app_name_key(AppKind::Connector, app_ref), n);
            }
        }
    }
    Ok(names)
}

/// A rule as `/team/rules` returns it: every field plus the sentence.
fn rule_json(r: &ApprovalRule, names: &HashMap<String, String>) -> Value {
    let app = match (r.app_kind, r.app_ref.as_deref()) {
        (Some(k), Some(app_ref)) => json!({
            "kind": k.as_str(), "ref": app_ref,
            "name": names.get(&app_name_key(k, app_ref)).cloned().unwrap_or_else(|| app_ref.to_string()),
        }),
        _ => Value::Null,
    };
    json!({
        "id": r.id,
        "text": rule_text(r, names),
        "enabled": r.enabled,
        "scope": { "kind": r.scope_kind.as_str(), "ref": r.scope_ref },
        "app": app,
        "tool": r.tool,
        "field": r.field,
        "op": r.op.as_str(),
        "value": r.value,
        "unit_label": r.unit_label,
        // No built-in rules exist: every rule is a stored row the admin can turn off or delete.
        "locked": false,
        "approver": { "kind": r.approver_kind.as_str(), "user_id": r.approver_user_id },
        "created_by": r.created_by,
        "created_at": r.created_at,
        "updated_at": r.updated_at,
    })
}

fn one_rule(conn: &Connection, v: &Viewer, r: &ApprovalRule) -> WriteResult {
    let names = rule_names(conn, v, std::slice::from_ref(r))?;
    Ok(json!({ "rule": rule_json(r, &names) }))
}

/// The rule, if it belongs to the viewer's org (solo: the viewer's own no-org rules).
fn rule_in_scope(conn: &Connection, v: &Viewer, rule_id: &str) -> Result<ApprovalRule, Fail> {
    let rule = team_store::get_rule(conn, rule_id)?;
    let ours = |r: &ApprovalRule| match v.org_id.as_deref() {
        Some(o) => r.org_id.as_deref() == Some(o),
        None => r.org_id.is_none() && r.owner_user_id == v.user_id,
    };
    rule.filter(ours).ok_or_else(|| Fail::new(StatusCode::NOT_FOUND, "rule_not_found", "That rule was not found."))
}

/// `GET /team/rules` → `{rules: [rule]}`. Every member reads them.
pub fn list_rules(conn: &Connection, v: &Viewer) -> WriteResult {
    let rules = team_store::list_rules(conn, v.org_id.as_deref(), &v.user_id)?;
    let names = rule_names(conn, v, &rules)?;
    Ok(json!({ "rules": rules.iter().map(|r| rule_json(r, &names)).collect::<Vec<_>>() }))
}

/// `POST /team/rules` → `{rule}`. Owner/admin.
pub fn create_rule(conn: &Connection, v: &Viewer, body: &Value) -> WriteResult {
    if !v.is_admin {
        return Err(owner_or_admin_required());
    }
    let mut fields = json!({});
    overlay_rule(&mut fields, body)?;
    let input = rule_input(conn, v, &fields, v.org_id.clone(), v.user_id.clone())?;
    let rule = team_store::create_rule(conn, &input, &v.user_id)?;
    one_rule(conn, v, &rule)
}

/// `PATCH /team/rules/:rule_id` → `{rule}`. Owner/admin. The body is merged
/// into the stored row (null clears a field), validated, then stored whole.
pub fn patch_rule(conn: &Connection, v: &Viewer, rule_id: &str, body: &Value) -> WriteResult {
    if !v.is_admin {
        return Err(owner_or_admin_required());
    }
    let existing = rule_in_scope(conn, v, rule_id)?;
    let mut fields = rule_fields(&existing);
    overlay_rule(&mut fields, body)?;
    let input = rule_input(conn, v, &fields, existing.org_id.clone(), existing.owner_user_id.clone())?;
    let rule = team_store::update_rule(conn, rule_id, &input)?
        .ok_or_else(|| Fail::new(StatusCode::NOT_FOUND, "rule_not_found", "That rule was not found."))?;
    one_rule(conn, v, &rule)
}

/// `DELETE /team/rules/:rule_id` → `{deleted: true}`. Owner/admin.
pub fn delete_rule(conn: &Connection, v: &Viewer, rule_id: &str) -> WriteResult {
    if !v.is_admin {
        return Err(owner_or_admin_required());
    }
    rule_in_scope(conn, v, rule_id)?;
    Ok(json!({ "deleted": team_store::delete_rule(conn, rule_id)? }))
}

async fn list_rules_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run_team(state, user, |conn, v| list_rules(conn, v)).await
}

async fn create_rule_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<Value>) -> Response {
    run_team(state, user, move |conn, v| create_rule(conn, v, &body)).await
}

async fn patch_rule_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(rule_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    run_team(state, user, move |conn, v| patch_rule(conn, v, &rule_id, &body)).await
}

async fn delete_rule_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(rule_id): Path<String>,
) -> Response {
    run_team(state, user, move |conn, v| delete_rule(conn, v, &rule_id)).await
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
        let map = map_json(&conn, &v, &cloud, &TierRead::skipped()).unwrap();
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
        let m2 = map_json(&conn, &other, &cloud, &TierRead::skipped()).unwrap();
        assert!(m2["asks"].as_array().unwrap().is_empty());
        let bot2 = m2["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "b:bot1").unwrap();
        assert_eq!(bot2["counts"]["asks"], 1);
    }

    #[test]
    fn team_map_jobs_show_the_waiting_bar_job_on_the_approvers_node() {
        let (conn, admin) = seeded();
        conn.execute("INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn1', 'emp1', 'Stripe', 'stripe', 'https://x')", []).unwrap();
        let no_args = json!({});
        let new_job = |bot: Option<&'static str>| bar_routes::NewJob {
            user_id: "emp1",
            bot_id: bot,
            app_kind: AppKind::Connector,
            app_ref: "conn1",
            tool: "refund_payment",
            args: &no_args,
            level: None,
            rule_id: None,
            approver_user_id: Some("owner1"),
        };
        let waiting = bar_routes::insert_job(&conn, &new_job(Some("bot1"))).unwrap();
        bar_routes::set_job_state(&conn, &waiting, JobState::WaitingApproval, &Default::default()).unwrap();
        let mine = bar_routes::insert_job(&conn, &new_job(None)).unwrap();
        bar_routes::set_job_state(&conn, &mine, JobState::Working, &Default::default()).unwrap();
        // Still in review: not active, not on the map.
        bar_routes::insert_job(&conn, &new_job(None)).unwrap();

        let map = map_json(&conn, &admin, &Ok(vec![]), &TierRead::skipped()).unwrap();
        let jobs = map["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 2);
        let w = jobs.iter().find(|j| j["job_id"] == waiting.as_str()).unwrap();
        assert_eq!(
            (w["actor"].as_str(), w["waiting_on"].as_str(), w["state"].as_str(), w["title"].as_str()),
            (Some("b:bot1"), Some("p:owner1"), Some("waiting_approval"), Some("refund payment"))
        );
        assert_eq!(w["app"]["name"], "Stripe");
        let m = jobs.iter().find(|j| j["job_id"] == mine.as_str()).unwrap();
        assert_eq!((m["actor"].as_str(), m["waiting_on"].clone()), (Some("p:emp1"), Value::Null));
        let node = |id: &str| map["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).unwrap().clone();
        assert_eq!(node("b:bot1")["counts"]["jobs"], 1);
        assert_eq!(node("p:owner1")["counts"]["jobs"], 1, "the waiting job shows on the approver's node");
        assert_eq!(node("p:emp1")["counts"]["jobs"], 1);
        assert_eq!(node("b:bot2")["counts"]["jobs"], 0);

        // A member who is neither the requester nor the approver sees counts, not the jobs.
        let other = Viewer { user_id: "emp2".into(), is_admin: false, ..admin.clone() };
        let m2 = map_json(&conn, &other, &Ok(vec![]), &TierRead::skipped()).unwrap();
        assert!(m2["jobs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn team_map_bot_nodes_carry_brain() {
        let (conn, admin) = seeded();
        conn.execute("UPDATE agents SET config = '{\"botBrain\":{\"runtime\":\"claude\",\"modelRef\":\"anthropic/claude-opus\"}}' WHERE id = 'bot1'", []).unwrap();
        let map = map_json(&conn, &admin, &Ok(vec![]), &TierRead::skipped()).unwrap();
        let brain = |id: &str| map["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).unwrap()["brain"].clone();
        assert_eq!(brain("b:bot1"), json!("claude-opus"));
        assert_eq!(brain("b:bot2"), json!("m"), "falls back to the agent row's model column");
    }

    #[test]
    fn team_asks_carry_the_routed_asks_app() {
        let (conn, admin) = seeded();
        conn.execute(
            "INSERT INTO cowork_approvals (id, user_id, content, source, dismissed, approver_user_id, bot_id)
             VALUES ('ap2', 'emp1', '{\"tool\":\"refund_payment\",\"app\":{\"kind\":\"vendor\",\"ref\":\"vap1\",\"name\":\"Linear\"}}', 'team-rule', 0, 'owner1', 'bot1')",
            [],
        )
        .unwrap();
        let map = map_json(&conn, &admin, &Ok(vec![]), &TierRead::skipped()).unwrap();
        let asks = map["asks"].as_array().unwrap();
        let routed = asks.iter().find(|a| a["id"] == "cowork:ap2").unwrap();
        assert_eq!(routed["app"], json!({ "kind": "vendor", "ref": "vap1", "name": "Linear", "icon": null }));
        // An ask with no app keeps `app: null`.
        assert_eq!(asks.iter().find(|a| a["id"] == "cowork:ap1").unwrap()["app"], Value::Null);
    }

    #[tokio::test]
    async fn team_map_partial_is_set_when_the_cloud_is_off() {
        let (conn, v) = seeded();
        let cloud = fetch_cloud_asks(&Off, "owner1").await;
        assert!(cloud.is_err());
        let map = map_json(&conn, &v, &cloud, &TierRead::skipped()).unwrap();
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
        let map = map_json(&conn, &v, &Err("off".into()), &TierRead::skipped()).unwrap();
        assert_eq!(map["grants"][0]["app"]["name"], "Notion");
    }
    // ---------------------------------------------------------------- lane 13 writes

    /// The cloud app shelf answering for `emp1` (the computer's paired owner).
    struct ShelfCloud(Value);
    #[async_trait]
    impl AccountCloud for ShelfCloud {
        async fn send(&self, m: &str, p: &str, _b: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
            assert_eq!((m, p), ("GET", CLOUD_APPS_PATH));
            Ok((200, self.0.to_string().into_bytes()))
        }
    }

    fn member(v: &Viewer, user: &str) -> Viewer {
        Viewer { user_id: user.into(), is_admin: false, ..v.clone() }
    }

    #[test]
    fn team_rules_member_cannot_edit_rules() {
        let (conn, admin) = seeded();
        let emp = member(&admin, "emp1");
        let body = json!({ "scope_kind": "org", "tool": "send_payment", "op": "gt", "field": "amount",
                           "value": 50000, "unit_label": "cents", "approver_kind": "owner" });
        let refused = create_rule(&conn, &emp, &body).unwrap_err();
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(refused.body["error"], "owner_or_admin_required");
        assert!(team_store::list_rules(&conn, Some("org1"), "emp1").unwrap().is_empty());

        // The owner can; the member still cannot change or delete it.
        let made = create_rule(&conn, &admin, &body).unwrap();
        let id = made["rule"]["id"].as_str().unwrap().to_string();
        assert_eq!(made["rule"]["text"], "Anyone · send payment · amount over 50000 cents needs the owner");
        assert_eq!(patch_rule(&conn, &emp, &id, &json!({ "enabled": false })).unwrap_err().status, StatusCode::FORBIDDEN);
        assert_eq!(delete_rule(&conn, &emp, &id).unwrap_err().status, StatusCode::FORBIDDEN);
        assert!(team_store::get_rule(&conn, &id).unwrap().unwrap().enabled);
        // Every member reads them.
        assert_eq!(list_rules(&conn, &emp).unwrap()["rules"].as_array().unwrap().len(), 1);

        // PATCH merges, and validation still runs on the merged row.
        let bad = patch_rule(&conn, &admin, &id, &json!({ "value": "lots" })).unwrap_err();
        assert_eq!((bad.status, bad.body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_rule")));
        conn.execute("INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn1', 'owner1', 'Stripe', 'stripe', 'https://x')", []).unwrap();
        let patched = patch_rule(&conn, &admin, &id, &json!({ "app_kind": "connector", "app_ref": "conn1", "value": 1000 })).unwrap();
        assert_eq!(patched["rule"]["value"], 1000);
        assert_eq!(patched["rule"]["field"], "amount");
        assert_eq!(patched["rule"]["text"], "Anyone · Stripe send payment · amount over 1000 cents needs the owner");
        assert_eq!(delete_rule(&conn, &admin, &id).unwrap()["deleted"], true);
    }

    #[test]
    fn team_people_reports_to_cycle_is_rejected() {
        let (conn, admin) = seeded();
        // emp1 already reports to owner1, so owner1 -> emp1 is a loop.
        let cycle = put_person(&conn, &admin, "owner1", &json!({ "reports_to_user_id": "emp1" })).unwrap_err();
        assert_eq!((cycle.status, cycle.body["error"].as_str()), (StatusCode::CONFLICT, Some("reports_to_cycle")));
        assert_eq!(team_store::get_reports_to(&conn, "org1", "owner1").unwrap(), None);

        let stranger = put_person(&conn, &admin, "emp1", &json!({ "reports_to_user_id": "nobody" })).unwrap_err();
        assert_eq!((stranger.status, stranger.body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("manager_not_member")));
        let not_member = put_person(&conn, &admin, "nobody", &json!({ "reports_to_user_id": null })).unwrap_err();
        assert_eq!((not_member.status, not_member.body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("not_member")));
        let emp = member(&admin, "emp1");
        assert_eq!(put_person(&conn, &emp, "emp1", &json!({ "reports_to_user_id": null })).unwrap_err().status, StatusCode::FORBIDDEN);

        let cleared = put_person(&conn, &admin, "emp1", &json!({ "reports_to_user_id": null })).unwrap();
        assert_eq!(cleared["reports_to_user_id"], Value::Null);
        assert_eq!(team_store::get_reports_to(&conn, "org1", "emp1").unwrap(), None);
    }

    #[tokio::test]
    async fn team_grants_above_ceiling_are_refused() {
        let (conn, admin) = seeded();
        let emp = member(&admin, "emp1");
        let shelf = json!({
            "yours": [{ "kind": "artifact", "ref": "art1", "name": "Quote builder", "icon": null, "provenance": "you",
                        "tier": "full", "granted_tier": "app", "installed": true, "pinned": false }],
            "servers": [{ "kind": "vendor", "ref": "ven1", "name": "Linear", "icon": null, "provenance": "server",
                          "tier": "app", "installed": true, "pinned": false }],
        });
        let tiers = TierRead::read(&ShelfCloud(shelf), Some("emp1".into())).await;
        let put = |v: &Viewer, kind: &str, app: &str, level: &str| {
            put_grant(&conn, v, &json!({ "bot_kind": "agent", "bot_id": "bot1", "app_kind": kind, "app_ref": app, "level": level }), &tiers)
        };

        // The owner installed art1 at app, so full is above the ceiling.
        let above = put(&emp, "artifact", "art1", "full").unwrap_err();
        assert_eq!(above.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(above.body["error"], "above_ceiling");
        assert_eq!(above.body["ceiling"], "app");
        // A vendor app never goes above app.
        let vendor = put(&emp, "vendor", "ven1", "full").unwrap_err();
        assert_eq!((vendor.body["error"].as_str(), vendor.body["ceiling"].as_str()), (Some("above_ceiling"), Some("app")));
        assert!(team_store::list_grants_for_bot(&conn, BotKind::Agent, "bot1").unwrap().is_empty());

        // The bot's owner (a plain member) sets a level at the ceiling.
        let ok = put(&emp, "artifact", "art1", "app").unwrap();
        assert_eq!(ok["grant"]["level"], "app");
        assert_eq!(ok["grant"]["ceiling"], "app");
        assert_eq!(ok["grant"]["app"]["name"], "Quote builder");

        // The map shows the real ceiling and app_tiers is no longer partial.
        let map = map_json(&conn, &admin, &Ok(vec![]), &tiers).unwrap();
        assert_eq!(map["grants"][0]["ceiling"], "app");
        assert!(!map["partial"].as_array().unwrap().iter().any(|p| p == PARTIAL_APP_TIERS));
    }

    #[tokio::test]
    async fn team_grants_503_when_the_tier_check_is_down() {
        let (conn, admin) = seeded();
        let tiers = TierRead::read(&Off, Some("emp1".into())).await;
        assert!(tiers.shelf.is_err());
        let down = put_grant(
            &conn,
            &admin,
            &json!({ "bot_kind": "agent", "bot_id": "bot1", "app_kind": "artifact", "app_ref": "art1", "level": "browser" }),
            &tiers,
        )
        .unwrap_err();
        assert_eq!(down.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(down.body["error"], "tier_check_unavailable");
        assert!(team_store::list_grants_for_bot(&conn, BotKind::Agent, "bot1").unwrap().is_empty());

        // A local connector needs no cloud read.
        conn.execute("INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn1', 'emp1', 'Notion', 'notion', 'https://x')", []).unwrap();
        let ok = put_grant(
            &conn,
            &admin,
            &json!({ "bot_kind": "agent", "bot_id": "bot1", "app_kind": "connector", "app_ref": "conn1", "level": "full" }),
            &TierRead::skipped(),
        )
        .unwrap();
        assert_eq!(ok["grant"]["ceiling"], "full");
        let map = map_json(&conn, &admin, &Ok(vec![]), &tiers).unwrap();
        assert!(map["partial"].as_array().unwrap().iter().any(|p| p == PARTIAL_APP_TIERS));
    }
}
