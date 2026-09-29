//! Project coordinator (spec P5, decision D1): one conversation per project
//! with Al, which turns a request into threads for the project's bots.
//!
//! Al plans; the system decides. The model proposes a JSON plan (or a route
//! for a follow-up); the server validates it deterministically before
//! anything runs:
//!
//! * steps may only use bots on the project's team (or, for a project with
//!   no team yet, the user's own bots), matched by id or display name;
//! * dependencies must reference other steps in the same plan, with no
//!   cycles; at most `MAX_STEPS` steps;
//! * a follow-up can only route to an open thread in this project.
//!
//! When the model is unavailable or proposes something invalid, the request
//! becomes one thread for the best-matching bot — never a silent drop.
//!
//! Execution: independent threads start at once (a kickoff turn with the
//! objective); dependent threads are `queued` and start when every thread
//! they wait on reaches review or done. When a thread's kickoff turn
//! finishes it moves to `review` and Al posts a short completion line in the
//! project chat. Al never does the work itself (AL_IMPLEMENTATION_SPEC §2).
//!
//! Canonical graph (P5.1/P5.6): a validated plan is also written to the rails
//! DAG — one node per thread, `blocked_by` edges for dependencies — and each
//! thread's status moves its node (RUNNING, DONE, BLOCKED). The threads stay
//! the execution record; the DAG is the graph the goal loop, WIH and the
//! rails views read. A DAG write failing never blocks the plan.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::thread_routes::{self, CreateThreadBody, ThreadRuntime, ThreadView, TodoItem};
use crate::AppState;

pub const MAX_STEPS: usize = 6;
const REPLY_CAP: usize = 600;

pub fn coordinator_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/projects/:project_id/messages", get(list_messages).post(post_message))
        .route("/projects/:project_id/start", post(start_project))
}

// ─── Runtime seam ───────────────────────────────────────────────────────────

/// Everything the coordinator needs from the outside world.
pub trait CoordinatorRuntime: ThreadRuntime {
    /// One-shot planning completion on `model` (provider, model) when given,
    /// else the platform default; `None` when the model is unavailable.
    fn plan(&self, system: &str, prompt: &str, model: Option<(String, String)>) -> impl Future<Output = Option<String>> + Send;
    /// Run a turn in a thread's session; returns the bot's reply text.
    fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> impl Future<Output = Result<String, String>> + Send;
    /// Write a plan to the canonical DAG: `steps` are (key, title, dependency
    /// keys). Returns the DAG id and each step key's node id.
    fn mirror_plan(
        &self,
        _goal: &str,
        _project_id: &str,
        _steps: &[(String, String, Vec<String>)],
    ) -> impl Future<Output = Option<(String, HashMap<String, String>)>> + Send {
        async { None }
    }
    /// Move a plan node to `to` (a rails status: RUNNING, DONE, BLOCKED).
    fn node_status(&self, _dag_id: &str, _node_id: &str, _from: &str, _to: &str) -> impl Future<Output = ()> + Send {
        async {}
    }
}

/// Rails status for a thread status; `None` leaves the node as it is.
pub fn dag_status(thread_status: &str) -> Option<&'static str> {
    match thread_status {
        "working" => Some("RUNNING"),
        "review" | "done" => Some("DONE"),
        "blocked" | "needs_you" => Some("BLOCKED"),
        "failed" => Some("FAILED"),
        _ => None,
    }
}

pub struct GizziCoordinator {
    pub state: Arc<AppState>,
}

impl ThreadRuntime for GizziCoordinator {
    async fn create_session(&self, bot_id: &str, bot_name: &str, title: &str, canonical: bool, thread_id: &str) -> Result<String, String> {
        crate::agent_session_routes::create_bot_thread_session(&self.state.db, bot_id, bot_name, title, canonical, Some(thread_id)).await
    }
    async fn seed(&self, session_id: &str, text: &str) -> Result<(), String> {
        crate::agent_session_routes::seed_session_message(&self.state.db, session_id, text).await
    }
    async fn handoff(&self, session_id: &str, reason: &str, context: &str, baton: Option<Value>) -> Result<(String, Value), String> {
        crate::thread_routes::GizziRuntime { db: self.state.db.clone() }.handoff(session_id, reason, context, baton).await
    }
    async fn successors(&self, session_id: &str) -> Vec<(String, String, Value)> {
        crate::thread_routes::GizziRuntime { db: self.state.db.clone() }.successors(session_id).await
    }
}

impl CoordinatorRuntime for GizziCoordinator {
    async fn plan(&self, system: &str, prompt: &str, model: Option<(String, String)>) -> Option<String> {
        crate::gizzi_completion::complete_ephemeral(prompt, Some(system), model.as_ref()).await
    }
    async fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn(&self.state.db, session_id, bot_id, text).await
    }
    async fn mirror_plan(&self, goal: &str, project_id: &str, steps: &[(String, String, Vec<String>)]) -> Option<(String, HashMap<String, String>)> {
        use allternit_commrails::DagMutation;
        let gate = &self.state.rails.gate;
        let (_, dag_id, root) = gate
            .plan_new(goal, Some(project_id.to_string()))
            .await
            .map_err(|e| warn!(project = %project_id, error = %e, "plan → DAG: create failed"))
            .ok()?;
        let nodes: HashMap<String, String> = steps
            .iter()
            .map(|(key, _, _)| (key.clone(), format!("n_{}", uuid::Uuid::new_v4().simple())))
            .collect();
        let mut mutations: Vec<DagMutation> = steps
            .iter()
            .map(|(key, title, _)| DagMutation::CreateNode {
                node_id: nodes[key].clone(),
                node_kind: "task".into(),
                title: title.clone(),
                parent_node_id: Some(root.clone()),
                execution_mode: "shared".into(),
            })
            .collect();
        for (key, _, deps) in steps {
            for dep in deps {
                if let Some(from) = nodes.get(dep) {
                    mutations.push(DagMutation::AddBlockedBy { from_node_id: from.clone(), to_node_id: nodes[key].clone() });
                }
            }
        }
        if !mutations.is_empty() {
            if let Err(e) = gate.mutate_with_decision(&dag_id, "coordinator plan", None, mutations).await {
                warn!(project = %project_id, error = %e, "plan → DAG: nodes failed");
                return None;
            }
        }
        Some((dag_id, nodes))
    }
    async fn node_status(&self, dag_id: &str, node_id: &str, from: &str, to: &str) {
        use allternit_commrails::DagMutation;
        let change = DagMutation::ChangeStatus { node_id: node_id.into(), from: from.into(), to: to.into(), reason: Some("bot thread".into()) };
        if let Err(e) = self.state.rails.gate.mutate_with_decision(dag_id, "thread status", None, vec![change]).await {
            warn!(dag = %dag_id, node = %node_id, error = %e, "thread status → DAG failed");
        }
    }
}

// ─── Plan model ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlanStep {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub objective: String,
    /// Bot id or display name.
    pub bot: String,
    #[serde(default, rename = "dependsOn")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub todo: Vec<String>,
    /// Optional spend cap for this thread, in USD (P8.1).
    #[serde(default, rename = "budgetUsd")]
    pub budget_usd: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Proposal {
    Plan { reply: String, steps: Vec<PlanStep> },
    Route {
        reply: String,
        #[serde(rename = "threadId")]
        thread_id: String,
    },
    Answer { reply: String },
}

#[derive(Debug, Clone)]
pub struct TeamBot {
    pub id: String,
    pub name: String,
    pub about: String,
}

#[derive(Debug, Clone)]
pub struct OpenThread {
    pub id: String,
    pub title: String,
    pub status: String,
    pub bot_name: String,
}

pub const PLANNER_SYSTEM: &str = "You are Al, the coordinator of a team of bots. You plan and delegate; you never do the work yourself. \
Reply with ONE JSON object and nothing else. Choose one:\n\
1. A new request: {\"action\":\"plan\",\"reply\":\"<one or two plain sentences to the user>\",\"steps\":[{\"key\":\"<short-id>\",\"title\":\"<3-6 words>\",\"objective\":\"<what done looks like>\",\"bot\":\"<exact bot name from the team>\",\"dependsOn\":[\"<key of a step that must finish first>\"],\"todo\":[\"<2-4 short steps>\"],\"budgetUsd\":<optional spend cap, only if the user gave one>}]}\n\
   Use the fewest steps that let independent work run in parallel (max 6). Only use bots from the team list.\n\
2. A follow-up that belongs to one open thread: {\"action\":\"route\",\"reply\":\"<one sentence>\",\"threadId\":\"<id from the open threads>\"}\n\
3. A question you can answer from what you already know about the project: {\"action\":\"answer\",\"reply\":\"<answer>\"}";

pub fn planner_prompt(project_title: &str, team: &[TeamBot], open: &[OpenThread], message: &str) -> String {
    let mut p = format!("Project: {project_title}\n\nTeam:\n");
    for b in team {
        p.push_str(&format!("- {} — {}\n", b.name, b.about));
    }
    if open.is_empty() {
        p.push_str("\nOpen threads: none\n");
    } else {
        p.push_str("\nOpen threads:\n");
        for t in open {
            p.push_str(&format!("- id {} · \"{}\" · {} · {}\n", t.id, t.title, t.bot_name, t.status));
        }
    }
    p.push_str(&format!("\nUser: {message}"));
    p
}

/// Pull the first JSON object out of a model reply (tolerates code fences).
pub fn parse_proposal(raw: &str) -> Option<Proposal> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    serde_json::from_str(&raw[start..=end]).ok()
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

fn resolve_bot<'a>(team: &'a [TeamBot], name_or_id: &str) -> Option<&'a TeamBot> {
    let n = norm(name_or_id);
    team.iter()
        .find(|b| b.id == name_or_id)
        .or_else(|| team.iter().find(|b| norm(&b.name) == n))
        .or_else(|| team.iter().find(|b| !n.is_empty() && (norm(&b.name).starts_with(&n) || n.starts_with(&norm(&b.name)))))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidStep {
    pub key: String,
    pub title: String,
    pub objective: String,
    pub bot_id: String,
    pub depends_on: Vec<String>,
    pub todo: Vec<String>,
    pub budget_usd: Option<f64>,
}

/// Deterministic checks on a proposed plan. Returns the steps to create, or
/// why the plan was rejected.
pub fn validate_plan(steps: &[PlanStep], team: &[TeamBot]) -> Result<Vec<ValidStep>, String> {
    if steps.is_empty() {
        return Err("empty plan".into());
    }
    if steps.len() > MAX_STEPS {
        return Err(format!("plan has {} steps; the limit is {MAX_STEPS}", steps.len()));
    }
    let keys: std::collections::HashSet<&str> = steps.iter().map(|s| s.key.as_str()).collect();
    if keys.len() != steps.len() {
        return Err("duplicate step keys".into());
    }
    let mut out = Vec::new();
    for s in steps {
        let bot = resolve_bot(team, &s.bot).ok_or_else(|| format!("\"{}\" is not on the team", s.bot))?;
        for d in &s.depends_on {
            if !keys.contains(d.as_str()) || d == &s.key {
                return Err(format!("step {} depends on unknown step {d}", s.key));
            }
        }
        if s.title.trim().is_empty() {
            return Err("a step has no title".into());
        }
        out.push(ValidStep {
            key: s.key.clone(),
            title: s.title.trim().chars().take(80).collect(),
            objective: if s.objective.trim().is_empty() { s.title.trim().to_string() } else { s.objective.trim().to_string() },
            bot_id: bot.id.clone(),
            depends_on: s.depends_on.clone(),
            todo: s.todo.iter().take(6).cloned().collect(),
            budget_usd: s.budget_usd.filter(|v| v.is_finite() && *v > 0.0),
        });
    }
    // Cycle check (Kahn).
    let mut remaining: Vec<&ValidStep> = out.iter().collect();
    let mut done = std::collections::HashSet::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|s| {
            if s.depends_on.iter().all(|d| done.contains(d.as_str())) {
                done.insert(s.key.as_str());
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            return Err("plan has a dependency cycle".into());
        }
    }
    Ok(out)
}

/// Fallback: one thread for the bot whose name/role best matches the text,
/// else the first team bot.
pub fn fallback_step(team: &[TeamBot], message: &str) -> Option<ValidStep> {
    let words: Vec<String> = message.split_whitespace().map(norm).filter(|w| w.len() > 3).collect();
    let score = |b: &TeamBot| {
        let hay = norm(&format!("{} {}", b.name, b.about));
        words.iter().filter(|w| hay.contains(w.as_str())).count()
    };
    let bot = team.iter().max_by_key(|b| score(b))?;
    let title: String = message
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'))
        .to_string();
    Some(ValidStep {
        key: "request".into(),
        title: if title.is_empty() { "Request".into() } else { title },
        objective: message.to_string(),
        bot_id: bot.id.clone(),
        depends_on: vec![],
        todo: vec![],
        budget_usd: None,
    })
}

// ─── Persistence ────────────────────────────────────────────────────────────

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn add_message(db: &DbHandle, project_id: &str, user_id: &str, role: &str, text: &str, payload: Value) -> rusqlite::Result<Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let ts = now();
    db.connect()?.execute(
        "INSERT INTO project_messages (id, project_id, user_id, role, text, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, project_id, user_id, role, text, payload.to_string(), ts],
    )?;
    Ok(json!({ "id": id, "role": role, "text": text, "payload": payload, "createdAt": ts }))
}

fn project_title(db: &DbHandle, project_id: &str, user_id: &str) -> rusqlite::Result<Option<String>> {
    db.connect()?
        .query_row(
            "SELECT title FROM cowork_projects WHERE id = ?1 AND user_id = ?2",
            params![project_id, user_id],
            |r| r.get(0),
        )
        .optional()
}

pub fn load_team(db: &DbHandle, project_id: &str, user_id: &str) -> rusqlite::Result<Vec<TeamBot>> {
    let conn = db.connect()?;
    let sql_team = "SELECT a.id, COALESCE(json_extract(a.config, '$.botProfile.displayName'), a.name),
                           TRIM(COALESCE(json_extract(a.config, '$.botProfile.tagline'), '') || ' ' || COALESCE(a.description, ''))
                    FROM project_bots p JOIN agents a ON a.id = p.bot_id
                    WHERE p.project_id = ?1 AND a.user_id = ?2 ORDER BY p.added_at";
    let map = |r: &rusqlite::Row<'_>| Ok(TeamBot { id: r.get(0)?, name: r.get(1)?, about: r.get(2)? });
    let team: Vec<TeamBot> = conn.prepare(sql_team)?.query_map(params![project_id, user_id], map)?.filter_map(Result::ok).collect();
    if !team.is_empty() {
        return Ok(team);
    }
    // No team yet: the user's bots are the pool; the ones Al uses join the team.
    let sql_all = "SELECT a.id, COALESCE(json_extract(a.config, '$.botProfile.displayName'), a.name),
                          TRIM(COALESCE(json_extract(a.config, '$.botProfile.tagline'), '') || ' ' || COALESCE(a.description, ''))
                   FROM agents a
                   WHERE a.user_id = ?1 AND (a.is_bot = 1 OR json_extract(a.config, '$.isBot') = 1)
                   ORDER BY a.created_at";
    let all = conn.prepare(sql_all)?.query_map(params![user_id], map)?.filter_map(Result::ok).collect();
    Ok(all)
}

fn open_threads(db: &DbHandle, project_id: &str) -> rusqlite::Result<Vec<OpenThread>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.status, COALESCE(json_extract(a.config, '$.botProfile.displayName'), a.name)
         FROM bot_threads t JOIN agents a ON a.id = t.bot_id
         WHERE t.project_id = ?1 AND t.incognito = 0 AND t.status NOT IN ('done', 'failed')
         ORDER BY t.last_activity_at DESC LIMIT 20",
    )?;
    let rows = stmt.query_map(params![project_id], |r| Ok(OpenThread { id: r.get(0)?, title: r.get(1)?, status: r.get(2)?, bot_name: r.get(3)? }))?;
    rows.collect()
}

fn set_status(db: &DbHandle, thread_id: &str, status: &str, status_line: Option<&str>, summary: Option<&str>) {
    if let Ok(conn) = db.connect() {
        let ts = now();
        let _ = conn.execute(
            "UPDATE bot_threads SET status = ?2, status_line = COALESCE(?3, status_line), summary = COALESCE(?4, summary),
                 started_at = CASE WHEN ?2 = 'working' AND started_at IS NULL THEN ?5 ELSE started_at END,
                 last_activity_at = ?5, updated_at = ?5
             WHERE id = ?1",
            params![thread_id, status, status_line, summary, ts],
        );
    }
}

/// Kickoff turns live in this process, so a restart ends them mid-turn and
/// their threads would show "working" forever with nothing running. At
/// startup, move every such thread to `blocked` (it lands in "Waiting on
/// you") and say how to pick it back up. Returns how many were moved.
pub fn interrupt_orphaned_turns(db: &DbHandle) -> usize {
    let Ok(conn) = db.connect() else { return 0 };
    conn.execute(
        "UPDATE bot_threads SET status = 'blocked',
             status_line = 'Stopped when Allternit restarted. Send a message in this thread to pick it up again.',
             last_activity_at = ?1, updated_at = ?1
         WHERE status = 'working'",
        params![now()],
    )
    .unwrap_or(0)
}

/// `set_status` plus the thread's node on the canonical DAG, when it has one.
async fn move_thread<R: CoordinatorRuntime>(db: &DbHandle, rt: &R, thread_id: &str, status: &str, status_line: Option<&str>, summary: Option<&str>) {
    let before: Option<(String, Option<String>, Option<String>)> = db.connect().ok().and_then(|c| {
        c.query_row("SELECT status, dag_id, dag_node_id FROM bot_threads WHERE id = ?1", params![thread_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .ok()
            .flatten()
    });
    set_status(db, thread_id, status, status_line, summary);
    if let Some((prev, Some(dag), Some(node))) = before {
        if let Some(to) = dag_status(status) {
            // Threads are created `working` (roots) or claimed `working`
            // (dependents) before their kickoff runs; on the graph that's READY.
            let from = if prev == "working" && status == "working" { "READY" } else { dag_status(&prev).unwrap_or("NEW") };
            if from != to {
                rt.node_status(&dag, &node, from, to).await;
            }
        }
    }
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let cut: String = s.chars().take(cap).collect();
    format!("{}…", cut.trim_end())
}

/// The first team bot's own (provider, model), when it has one.
pub fn team_model(db: &DbHandle, team: &[TeamBot]) -> Option<(String, String)> {
    let conn = db.connect().ok()?;
    team.iter().find_map(|b| {
        conn.query_row(
            "SELECT provider, model FROM agents WHERE id = ?1",
            params![b.id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .ok()
        .and_then(|(p, m)| match (p, m) {
            (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => Some((p, m)),
            _ => None,
        })
    })
}

// ─── Coordinate ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Outcome {
    pub reply: Value,
    /// Threads to kick off now (independent steps or a routed follow-up).
    pub start: Vec<(String, String)>, // (thread_id, kickoff text)
}

/// Handle one user message: record it, get a proposal, validate, create
/// threads. Starting threads is separate (`start_threads`) so the HTTP
/// handler can answer at once.
pub async fn coordinate<R: CoordinatorRuntime>(db: &DbHandle, rt: &R, user_id: &str, project_id: &str, message: &str) -> Result<Outcome, String> {
    let title = project_title(db, project_id, user_id).map_err(|e| e.to_string())?.ok_or("project not found")?;
    add_message(db, project_id, user_id, "user", message, json!({})).map_err(|e| e.to_string())?;
    let team = load_team(db, project_id, user_id).map_err(|e| e.to_string())?;
    if team.is_empty() {
        let reply = add_message(db, project_id, user_id, "coordinator", "This project has no bots yet. Create a bot or add one to the team, then send the request again.", json!({"kind": "no_team"}))
            .map_err(|e| e.to_string())?;
        return Ok(Outcome { reply, start: vec![] });
    }
    let open = open_threads(db, project_id).map_err(|e| e.to_string())?;

    // Plan on a model the team already runs on — the platform default may
    // be a provider that isn't set up (live check 2026-09-27: every plan fell
    // back because the default pointed at an unconfigured provider).
    let model = team_model(db, &team);
    let proposal = rt
        .plan(PLANNER_SYSTEM, &planner_prompt(&title, &team, &open, message), model)
        .await
        .as_deref()
        .and_then(parse_proposal);

    // Route a follow-up to one open thread.
    if let Some(Proposal::Route { reply, thread_id }) = &proposal {
        if let Some(t) = open.iter().find(|t| &t.id == thread_id) {
            let text = truncate(reply, REPLY_CAP);
            let msg = add_message(db, project_id, user_id, "coordinator", &text, json!({"kind": "routed", "threadId": t.id, "threadTitle": t.title}))
                .map_err(|e| e.to_string())?;
            let kickoff = format!("[forwarded from project chat] {message}");
            return Ok(Outcome { reply: msg, start: vec![(t.id.clone(), kickoff)] });
        }
    }
    if let Some(Proposal::Answer { reply }) = &proposal {
        let msg = add_message(db, project_id, user_id, "coordinator", &truncate(reply, REPLY_CAP), json!({"kind": "answer"})).map_err(|e| e.to_string())?;
        return Ok(Outcome { reply: msg, start: vec![] });
    }

    let (steps, reply_text, planned) = match &proposal {
        Some(Proposal::Plan { reply, steps }) => match validate_plan(steps, &team) {
            Ok(v) => (v, truncate(reply, REPLY_CAP), true),
            Err(why) => {
                warn!(project = %project_id, %why, "coordinator plan rejected; using fallback");
                (fallback_step(&team, message).into_iter().collect(), String::new(), false)
            }
        },
        _ => (fallback_step(&team, message).into_iter().collect(), String::new(), false),
    };

    let graph = rt
        .mirror_plan(
            message,
            project_id,
            &steps.iter().map(|s| (s.key.clone(), s.title.clone(), s.depends_on.clone())).collect::<Vec<_>>(),
        )
        .await;

    let mut created: std::collections::HashMap<String, ThreadView> = std::collections::HashMap::new();
    let mut start = Vec::new();
    for step in &steps {
        let deps: Vec<String> = step.depends_on.iter().filter_map(|k| created.get(k).map(|t| t.id.clone())).collect();
        let body = CreateThreadBody {
            bot_id: step.bot_id.clone(),
            title: step.title.clone(),
            project_id: Some(project_id.to_string()),
            parent_thread_id: None,
            kind: "task".into(),
            incognito: false,
            objective: Some(step.objective.clone()),
            success_criteria: None,
            status: Some(if deps.is_empty() { "working".into() } else { "queued".into() }),
            todo: step.todo.iter().map(|t| TodoItem { text: t.clone(), state: "pending".into() }).collect(),
            created_by: Some("coordinator".into()),
            origin: Some(json!({ "projectMessage": message })),
            session_id: None,
            depends_on: deps.clone(),
        };
        let thread = thread_routes::create(db, rt, user_id, body).await?;
        if let (Some(usd), Some(session)) = (step.budget_usd, thread.current_session_id.as_deref()) {
            if let Err(e) = crate::spend_limits::set_thread_budget_for(session, Some(usd)).await {
                tracing::warn!(error = %e, thread = %thread.id, "couldn't set the plan's thread budget");
            }
        }
        if let Some((dag, nodes)) = &graph {
            if let (Some(node), Ok(conn)) = (nodes.get(&step.key), db.connect()) {
                let _ = conn.execute("UPDATE bot_threads SET dag_id = ?2, dag_node_id = ?3 WHERE id = ?1", params![thread.id, dag, node]);
            }
        }
        if deps.is_empty() {
            start.push((thread.id.clone(), kickoff_text(&title, &thread)));
        }
        created.insert(step.key.clone(), thread);
    }
    let ids: Vec<Value> = steps
        .iter()
        .filter_map(|s| created.get(&s.key))
        .map(|t| json!({"id": t.id, "title": t.title, "botId": t.bot_id, "status": t.status}))
        .collect();
    let text = if planned && !reply_text.is_empty() {
        reply_text
    } else if ids.len() == 1 {
        format!("Started a thread for this: {}.", created.values().next().map(|t| t.title.as_str()).unwrap_or("request"))
    } else {
        format!("Split this into {} threads.", ids.len())
    };
    let msg = add_message(db, project_id, user_id, "coordinator", &text, json!({"kind": "fanout", "threads": ids, "planned": planned, "dagId": graph.as_ref().map(|g| g.0.clone())}))
        .map_err(|e| e.to_string())?;
    Ok(Outcome { reply: msg, start })
}

pub fn kickoff_text(project_title: &str, t: &ThreadView) -> String {
    let mut s = format!("[coordinator: {project_title}] {}\n\nObjective: {}", t.title, t.objective.as_deref().unwrap_or(&t.title));
    if !t.todo.is_empty() {
        s.push_str("\n\nPlan:");
        for i in &t.todo {
            s.push_str(&format!("\n- {}", i.text));
        }
    }
    s.push_str("\n\nWork on this now and report what you did and what's left.");
    if !t.todo.is_empty() {
        s.push_str("\n\nEnd your report with this checklist, marking [x] only for steps you actually finished:\nChecklist:");
        for (n, i) in t.todo.iter().enumerate() {
            s.push_str(&format!("\n[ ] {}. {}", n + 1, i.text));
        }
    }
    s
}

/// Ticks the thread's plan from the checklist its bot ends the report with
/// (`[x] 2. …`). Steps are matched by number, then by text; a step the bot
/// didn't mark stays as it was, so nothing is ticked on a guess.
pub fn tick_todo(todo: &[TodoItem], reply: &str) -> Vec<TodoItem> {
    let mut out = todo.to_vec();
    for line in reply.lines() {
        let l = line.trim().trim_start_matches(['-', '*']).trim_start();
        let Some(rest) = l.strip_prefix('[') else { continue };
        let mut chars = rest.chars();
        let mark = chars.next();
        if chars.next() != Some(']') {
            continue;
        }
        let done = matches!(mark, Some('x' | 'X' | '✓' | '✔'));
        if !done && mark != Some(' ') {
            continue;
        }
        let body = chars.as_str().trim();
        let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
        let text = body[digits.len()..].trim_start_matches(['.', ')']).trim();
        let idx = digits.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= out.len()).map(|n| n - 1)
            .or_else(|| out.iter().position(|i| norm(&i.text) == norm(text)));
        if let Some(i) = idx {
            out[i].state = if done { "done" } else { "pending" }.into();
        }
    }
    out
}

/// The report's first line worth showing on its own: skips bare labels like
/// "**Caption:**" and the trailing checklist, and drops markdown emphasis.
pub fn status_line_of(reply: &str) -> Option<String> {
    reply
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim().replace("**", "").replace("__", ""))
        .map(|l| l.trim().to_string())
        .find(|l| !l.is_empty() && !l.ends_with(':') && !l.starts_with('[') && l.split_whitespace().count() >= 3)
        .or_else(|| reply.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string))
        .map(|l| truncate(&l, 140))
}

fn set_todo(db: &DbHandle, thread_id: &str, todo: &[TodoItem]) {
    if let (Ok(conn), Ok(json)) = (db.connect(), serde_json::to_string(todo)) {
        let _ = conn.execute("UPDATE bot_threads SET todo = ?2 WHERE id = ?1", params![thread_id, json]);
    }
}

/// Run kickoff turns. Every thread in a wave starts at once (independent
/// work runs in parallel, as the plan promises); each finished thread moves
/// to review, Al reports it, and dependents whose inputs are all ready form
/// the next wave. One slow or hung turn no longer holds the others back.
pub async fn start_threads<R: CoordinatorRuntime>(db: &DbHandle, rt: &R, user_id: &str, project_id: &str, start: Vec<(String, String)>) {
    let mut wave = start;
    while !wave.is_empty() {
        let next = futures::future::join_all(
            wave.into_iter().map(|(thread_id, text)| run_kickoff(db, rt, user_id, project_id, thread_id, text)),
        )
        .await;
        let mut seen = std::collections::HashSet::new();
        wave = next.into_iter().flatten().filter(|(id, _)| seen.insert(id.clone())).collect();
    }
    maybe_synthesize(db, rt, user_id, project_id).await;
}

/// One thread's kickoff turn; returns the dependents it made ready.
async fn run_kickoff<R: CoordinatorRuntime>(db: &DbHandle, rt: &R, user_id: &str, project_id: &str, thread_id: String, text: String) -> Vec<(String, String)> {
    let Ok(Some(t)) = thread_routes::load_view(db, &thread_id) else { return vec![] };
    let Some(session) = t.current_session_id.clone() else { return vec![] };
    move_thread(db, rt, &t.id, "working", Some("Working on it"), None).await;
    match rt.send_turn(&session, &t.bot_id, &text).await {
        Ok(reply) => {
            let line = status_line_of(&reply);
            if !t.todo.is_empty() {
                set_todo(db, &t.id, &tick_todo(&t.todo, &reply));
            }
            move_thread(db, rt, &t.id, "review", line.as_deref(), Some(&truncate(&reply, 1200))).await;
            let _ = add_message(
                db,
                project_id,
                user_id,
                "coordinator",
                &format!("The {} thread has an update ready for you{}", t.title, line.as_ref().map(|l| format!(": {l}")).unwrap_or_else(|| ".".into())),
                json!({"kind": "completed", "threadId": t.id, "threadTitle": t.title}),
            );
            ready_dependents(db, project_id, &t.id)
        }
        Err(e) => {
            move_thread(db, rt, &t.id, "blocked", Some(&truncate(&e, 140)), None).await;
            let _ = add_message(
                db,
                project_id,
                user_id,
                "coordinator",
                &format!("The {} thread is blocked: {}", t.title, truncate(&e, 200)),
                json!({"kind": "blocked", "threadId": t.id, "threadTitle": t.title}),
            );
            vec![]
        }
    }
}

pub const SYNTHESIS_SYSTEM: &str = "You are Al, the coordinator. Every thread in this project has reported back. \
Write the wrap-up the user reads first: what was decided and produced, the key numbers exactly as the threads gave them, \
and anything still open or needing their call. 3-8 short lines, plain text, no headings, no filler.";

/// When every task thread in the project is back (review or done), Al posts
/// one synthesis of what came back (P5.4). Again only after newer results.
pub async fn maybe_synthesize<R: CoordinatorRuntime>(db: &DbHandle, rt: &R, user_id: &str, project_id: &str) -> Option<Value> {
    let (threads, last_synthesis): (Vec<(String, String, Option<String>, String)>, Option<String>) = {
        let conn = db.connect().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT title, status, summary, updated_at FROM bot_threads
                 WHERE project_id = ?1 AND kind = 'task' AND incognito = 0",
            )
            .ok()?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .ok()?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        let last = conn
            .query_row(
                "SELECT MAX(created_at) FROM project_messages WHERE project_id = ?1 AND json_extract(payload, '$.kind') = 'synthesis'",
                params![project_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        (rows, last)
    };
    if threads.len() < 2 || !threads.iter().all(|t| t.1 == "review" || t.1 == "done") {
        return None;
    }
    let newest = threads.iter().map(|t| t.3.as_str()).max()?;
    if last_synthesis.as_deref().map_or(false, |l| l >= newest) {
        return None;
    }
    let title = project_title(db, project_id, user_id).ok().flatten().unwrap_or_default();
    let team = load_team(db, project_id, user_id).unwrap_or_default();
    let mut prompt = format!("Project: {title}\n\nWhat each thread reported:\n");
    for (t, _, summary, _) in &threads {
        prompt.push_str(&format!("\n## {t}\n{}\n", summary.as_deref().unwrap_or("(no report)")));
    }
    let text = rt.plan(SYNTHESIS_SYSTEM, &prompt, team_model(db, &team)).await?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    add_message(db, project_id, user_id, "coordinator", text, json!({ "kind": "synthesis", "threads": threads.len() })).ok()
}

/// Queued threads in the project whose dependencies are all in review/done.
pub fn ready_dependents(db: &DbHandle, project_id: &str, finished: &str) -> Vec<(String, String)> {
    let Ok(conn) = db.connect() else { return vec![] };
    let title: String = conn
        .query_row("SELECT title FROM cowork_projects WHERE id = ?1", params![project_id], |r| r.get(0))
        .unwrap_or_default();
    let candidates: Vec<String> = conn
        .prepare(
            "SELECT d.thread_id FROM bot_thread_deps d JOIN bot_threads t ON t.id = d.thread_id
             WHERE d.depends_on = ?1 AND t.status = 'queued'",
        )
        .and_then(|mut s| s.query_map(params![finished], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in candidates {
        let blocked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_thread_deps d JOIN bot_threads t ON t.id = d.depends_on
                 WHERE d.thread_id = ?1 AND t.status NOT IN ('review', 'done')",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        if blocked == 0 {
            // Claim: only one caller moves it out of queued.
            let claimed = conn
                .execute("UPDATE bot_threads SET status = 'working' WHERE id = ?1 AND status = 'queued'", params![id])
                .unwrap_or(0);
            if claimed == 1 {
                if let Ok(Some(t)) = thread_routes::load_view(db, &id) {
                    out.push((id.clone(), kickoff_text(&title, &t)));
                }
            }
        }
    }
    out
}

// ─── Handlers ───────────────────────────────────────────────────────────────

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

async fn list_messages(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<Vec<Value>>> {
        if project_title(&db, &project_id, &uid)?.is_none() {
            return Ok(None);
        }
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, text, payload, created_at FROM project_messages WHERE project_id = ?1 ORDER BY created_at LIMIT 500",
        )?;
        let rows = stmt.query_map(params![project_id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "role": r.get::<_, String>(1)?,
                "text": r.get::<_, String>(2)?,
                "payload": r.get::<_, Option<String>>(3)?.and_then(|p| serde_json::from_str::<Value>(&p).ok()).unwrap_or(Value::Null),
                "createdAt": r.get::<_, String>(4)?,
            }))
        })?;
        Ok(Some(rows.filter_map(Result::ok).collect()))
    })
    .await;
    match res {
        Ok(Ok(Some(messages))) => Json(json!({ "messages": messages })).into_response(),
        Ok(Ok(None)) => err(StatusCode::NOT_FOUND, "project not found"),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    }
}

#[derive(Debug, Deserialize)]
struct PostMessageBody {
    text: String,
}

async fn post_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
    Json(body): Json<PostMessageBody>,
) -> Response {
    if body.text.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "text is required");
    }
    let rt = GizziCoordinator { state: state.clone() };
    match coordinate(&state.db, &rt, &user.user_id, &project_id, body.text.trim()).await {
        Ok(outcome) => {
            if !outcome.start.is_empty() {
                let state2 = state.clone();
                let uid = user.user_id.clone();
                let pid = project_id.clone();
                tokio::spawn(async move {
                    let rt = GizziCoordinator { state: state2.clone() };
                    start_threads(&state2.db, &rt, &uid, &pid, outcome.start).await;
                });
            }
            (StatusCode::CREATED, Json(json!({ "message": outcome.reply }))).into_response()
        }
        Err(e) if e.contains("not found") => err(StatusCode::NOT_FOUND, e),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// Why a start request was refused.
#[derive(Debug, PartialEq)]
pub enum StartRefusal {
    ProjectNotFound,
    BadThread(String),
    Empty,
    Db,
}

/// Validates a request to start threads that already exist in a project
/// (a template lays out its plan, then starts the roots here) and claims
/// each one: `working` now, so the caller sees it move at once and a second
/// request can't run the same kickoff twice. Returns the (thread, kickoff
/// text) pairs to hand to `start_threads`, and the ids skipped because they
/// were already working. Every thread must belong to this project, which
/// must belong to the user, and have a session to run in; one bad thread
/// refuses the whole request, so nothing starts half-way.
pub fn accept_start(
    db: &DbHandle,
    user_id: &str,
    project_id: &str,
    requested: Vec<(String, String)>,
) -> Result<(Vec<(String, String)>, Vec<String>), StartRefusal> {
    let title = match project_title(db, project_id, user_id) {
        Ok(Some(t)) => t,
        Ok(None) => return Err(StartRefusal::ProjectNotFound),
        Err(_) => return Err(StartRefusal::Db),
    };
    let mut seen = std::collections::HashSet::new();
    let requested: Vec<(String, String)> = requested.into_iter().filter(|(id, _)| seen.insert(id.clone())).collect();
    if requested.is_empty() {
        return Err(StartRefusal::Empty);
    }
    let conn = db.connect().map_err(|_| StartRefusal::Db)?;
    let mut views = Vec::with_capacity(requested.len());
    for (id, text) in requested {
        let owner: Option<(Option<String>, String)> = conn
            .query_row("SELECT project_id, user_id FROM bot_threads WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|_| StartRefusal::Db)?;
        match owner {
            Some((Some(p), u)) if p == project_id && u == user_id => {}
            _ => return Err(StartRefusal::BadThread(format!("thread {id} is not in this project"))),
        }
        let view = thread_routes::load_view(db, &id).map_err(|_| StartRefusal::Db)?.ok_or(StartRefusal::Db)?;
        if view.current_session_id.is_none() {
            return Err(StartRefusal::BadThread(format!("thread {id} has no session to run in")));
        }
        views.push((view, text));
    }
    let mut start = Vec::new();
    let mut skipped = Vec::new();
    for (view, text) in views {
        let claimed = conn
            .execute("UPDATE bot_threads SET status = 'working' WHERE id = ?1 AND status <> 'working'", params![view.id])
            .map_err(|_| StartRefusal::Db)?;
        if claimed == 1 {
            let text = if text.trim().is_empty() { kickoff_text(&title, &view) } else { text };
            start.push((view.id, text));
        } else {
            skipped.push(view.id);
        }
    }
    Ok((start, skipped))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartThread {
    thread_id: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct StartBody {
    threads: Vec<StartThread>,
}

/// `POST /projects/:project_id/start` — start threads that already exist in
/// the project through the coordinator: they run in parallel, move to review
/// (or blocked), tick their checklists, Al reports each one, ready dependents
/// start after them and Al posts the wrap-up. Returns 202 at once; the turns
/// run in the background, so the caller never waits on a model.
async fn start_project(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
    Json(body): Json<StartBody>,
) -> Response {
    let requested: Vec<(String, String)> = body.threads.into_iter().map(|t| (t.thread_id, t.text)).collect();
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let pid = project_id.clone();
    let accepted = tokio::task::spawn_blocking(move || accept_start(&db, &uid, &pid, requested)).await;
    let (start, skipped) = match accepted {
        Ok(Ok(v)) => v,
        Ok(Err(StartRefusal::ProjectNotFound)) => return err(StatusCode::NOT_FOUND, "project not found"),
        Ok(Err(StartRefusal::BadThread(m))) => return err(StatusCode::BAD_REQUEST, m),
        Ok(Err(StartRefusal::Empty)) => return err(StatusCode::BAD_REQUEST, "threads is required"),
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    };
    let ids: Vec<String> = start.iter().map(|(id, _)| id.clone()).collect();
    if !start.is_empty() {
        let state2 = state.clone();
        let uid = user.user_id.clone();
        tokio::spawn(async move {
            let rt = GizziCoordinator { state: state2.clone() };
            start_threads(&state2.db, &rt, &uid, &project_id, start).await;
        });
    }
    (StatusCode::ACCEPTED, Json(json!({ "accepted": ids, "skipped": skipped }))).into_response()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        plan: Mutex<Option<String>>,
        turns: Mutex<Vec<(String, String)>>,
        fail_turns: bool,
        sessions: Mutex<usize>,
        graph: Mutex<Vec<(String, String, Vec<String>)>>,
        moves: Mutex<Vec<(String, String, String)>>,
        /// When set, every kickoff turn waits here: the test only finishes
        /// if the independent turns are in flight at the same time.
        rendezvous: Option<Arc<tokio::sync::Barrier>>,
    }

    impl ThreadRuntime for Fake {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, _id: &str) -> Result<String, String> {
            let mut n = self.sessions.lock().unwrap();
            *n += 1;
            Ok(format!("sess-{n}"))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, _s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Err("the coordinator does not hand off".into())
        }
    }

    impl CoordinatorRuntime for Fake {
        async fn plan(&self, _s: &str, _p: &str, _m: Option<(String, String)>) -> Option<String> {
            self.plan.lock().unwrap().clone()
        }
        async fn send_turn(&self, s: &str, _b: &str, t: &str) -> Result<String, String> {
            if self.fail_turns {
                return Err("provider_rate_limit".into());
            }
            if let Some(b) = &self.rendezvous {
                b.wait().await;
            }
            self.turns.lock().unwrap().push((s.into(), t.into()));
            Ok("Found three prices.\nDetails follow.\n\nChecklist:\n[x] 1. Pull prices\n[ ] 2. Compare".into())
        }
        async fn mirror_plan(&self, _g: &str, _p: &str, steps: &[(String, String, Vec<String>)]) -> Option<(String, HashMap<String, String>)> {
            *self.graph.lock().unwrap() = steps.to_vec();
            Some(("dag_1".into(), steps.iter().map(|(k, _, _)| (k.clone(), format!("node-{k}"))).collect()))
        }
        async fn node_status(&self, _d: &str, node: &str, from: &str, to: &str) {
            self.moves.lock().unwrap().push((node.into(), from.into(), to.into()));
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-coord-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        for (id, name, tag) in [("scout", "Scout", "Researcher"), ("ledger", "Ledger", "Finance analyst"), ("forge", "Forge", "Engineer")] {
            conn.execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'u', ?2, 'm', 'p', 1, ?3)",
                params![id, name.to_lowercase(), json!({"isBot": true, "botProfile": {"displayName": name, "tagline": tag}}).to_string()],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO cowork_projects (id, user_id, title) VALUES ('p1', 'u', 'Cloud pricing launch')", []).unwrap();
        state
    }

    fn team() -> Vec<TeamBot> {
        vec![
            TeamBot { id: "scout".into(), name: "Scout".into(), about: "Researcher".into() },
            TeamBot { id: "ledger".into(), name: "Ledger".into(), about: "Finance analyst".into() },
        ]
    }

    fn step(key: &str, bot: &str, deps: &[&str]) -> PlanStep {
        PlanStep { key: key.into(), title: format!("Step {key}"), objective: String::new(), bot: bot.into(), depends_on: deps.iter().map(|s| s.to_string()).collect(), todo: vec![], budget_usd: None }
    }

    #[test]
    fn a_plan_step_can_carry_a_budget() {
        let steps: Vec<PlanStep> = serde_json::from_value(json!([
            {"key": "a", "title": "Price it", "bot": "Scout", "budgetUsd": 5.0},
            {"key": "b", "title": "Page", "bot": "Scout", "budgetUsd": -1}
        ]))
        .unwrap();
        let valid = validate_plan(&steps, &team()).unwrap();
        assert_eq!(valid[0].budget_usd, Some(5.0));
        assert_eq!(valid[1].budget_usd, None, "nonsense budgets are dropped");
    }

    #[test]
    fn validation_enforces_team_deps_cycles_and_size() {
        assert!(validate_plan(&[step("a", "Scout", &[]), step("b", "ledger", &["a"])], &team()).is_ok());
        assert!(validate_plan(&[step("a", "Pixel", &[])], &team()).unwrap_err().contains("not on the team"));
        assert!(validate_plan(&[step("a", "Scout", &["zz"])], &team()).unwrap_err().contains("unknown step"));
        assert!(validate_plan(&[step("a", "Scout", &["b"]), step("b", "Scout", &["a"])], &team()).unwrap_err().contains("cycle"));
        let many: Vec<PlanStep> = (0..7).map(|i| step(&i.to_string(), "Scout", &[])).collect();
        assert!(validate_plan(&many, &team()).unwrap_err().contains("limit"));
    }

    #[test]
    fn parses_fenced_json_and_picks_fallback_bot() {
        let p = parse_proposal("```json\n{\"action\":\"answer\",\"reply\":\"Monday.\"}\n```").unwrap();
        assert_eq!(p, Proposal::Answer { reply: "Monday.".into() });
        let f = fallback_step(&team(), "Work out our finance numbers for Q4").unwrap();
        assert_eq!(f.bot_id, "ledger");
        let t = fallback_step(&team(), "Reply with the single word OK.").unwrap();
        assert_eq!(t.title, "Reply with the single word OK");
    }

    #[tokio::test]
    async fn a_restart_moves_stranded_working_threads_to_waiting_on_you() {
        let state = setup("restart").await;
        let rt = Fake::default();
        let out = coordinate(&state.db, &rt, "u", "p1", "Chase the flour supplier").await.unwrap();
        let id = out.start[0].0.clone();
        set_status(&state.db, &id, "working", Some("Working on it"), None);
        assert_eq!(interrupt_orphaned_turns(&state.db), 1);
        let t = thread_routes::load_view(&state.db, &id).unwrap().unwrap();
        assert_eq!(t.status, "blocked");
        assert!(t.status_line.as_deref().unwrap_or("").contains("restarted"));
        assert_eq!(interrupt_orphaned_turns(&state.db), 0);
    }

    #[tokio::test]
    async fn independent_threads_run_at_the_same_time() {
        let state = setup("parallel").await;
        let rt = Fake { rendezvous: Some(Arc::new(tokio::sync::Barrier::new(2))), ..Fake::default() };
        *rt.plan.lock().unwrap() = Some(json!({
            "action": "plan",
            "reply": "Two threads.",
            "steps": [
                {"key": "flour", "title": "Find backup flour supplier", "objective": "A supplier who can deliver Friday", "bot": "Scout"},
                {"key": "prices", "title": "Update chalkboard prices", "objective": "New spring prices", "bot": "Ledger"}
            ]
        }).to_string());
        let out = coordinate(&state.db, &rt, "u", "p1", "flour is late, and the chalkboard needs spring prices").await.unwrap();
        assert_eq!(out.start.len(), 2);
        // Run one at a time, the first turn waits at the barrier forever.
        tokio::time::timeout(std::time::Duration::from_secs(5), start_threads(&state.db, &rt, "u", "p1", out.start))
            .await
            .expect("independent kickoff turns must run concurrently");
        assert_eq!(rt.turns.lock().unwrap().len(), 2);
    }

    #[test]
    fn checklist_ticks_by_number_or_text_and_never_guesses() {
        let todo: Vec<TodoItem> = ["Pull prices", "Compare", "Write it up"]
            .iter()
            .map(|t| TodoItem { text: t.to_string(), state: "pending".into() })
            .collect();
        let states = |r: &str| tick_todo(&todo, r).into_iter().map(|i| i.state).collect::<Vec<_>>();
        assert_eq!(states("Done.\nChecklist:\n[x] 1. Pull prices\n- [X] 2) Compare\n[ ] 3. Write it up"), ["done", "done", "pending"]);
        assert_eq!(states("- [x] **write it up**"), ["pending", "pending", "done"], "text match ignores case and emphasis");
        assert_eq!(states("[x] Write it up\n[x] 9. Nope\n[?] 1. Pull prices"), ["pending", "pending", "done"]);
        assert_eq!(states("No checklist at all."), ["pending", "pending", "pending"]);
    }

    #[test]
    fn status_line_skips_bare_labels() {
        assert_eq!(status_line_of("**Caption:**\n\nSpring is here: our rhubarb galette is back.").as_deref(), Some("Spring is here: our rhubarb galette is back."));
        assert_eq!(status_line_of("## Recommended name: Bloom Box").as_deref(), Some("Recommended name: Bloom Box"));
        assert_eq!(status_line_of("Done").as_deref(), Some("Done"));
        assert_eq!(status_line_of("  \n"), None);
    }

    #[tokio::test]
    async fn plan_fans_out_starts_roots_and_chains_dependents() {
        let state = setup("plan").await;
        let rt = Fake::default();
        *rt.plan.lock().unwrap() = Some(json!({
            "action": "plan",
            "reply": "On it. Three threads; I'll flag anything that needs you.",
            "steps": [
                {"key": "research", "title": "Research cloud pricing", "objective": "Compare 6 providers", "bot": "Scout", "todo": ["Pull prices", "Compare"]},
                {"key": "econ", "title": "GPU unit economics", "objective": "Cost per GPU-hour", "bot": "Ledger"},
                {"key": "engine", "title": "Pricing engine", "objective": "Wire prices", "bot": "Forge", "dependsOn": ["econ"]}
            ]
        }).to_string());

        let out = coordinate(&state.db, &rt, "u", "p1", "Build the cloud pricing and launch page").await.unwrap();
        assert_eq!(out.reply["payload"]["kind"], "fanout");
        assert_eq!(out.reply["payload"]["threads"].as_array().unwrap().len(), 3);
        assert_eq!(out.reply["text"], "On it. Three threads; I'll flag anything that needs you.");
        assert_eq!(out.start.len(), 2, "only independent steps start now");
        assert!(out.start.iter().any(|(_, t)| t.contains("Plan:\n- Pull prices")));
        assert!(out.start.iter().any(|(_, t)| t.contains("Checklist:\n[ ] 1. Pull prices\n[ ] 2. Compare")));

        start_threads(&state.db, &rt, "u", "p1", out.start).await;
        // Both roots ran; economics finishing started the engine thread too.
        assert_eq!(rt.turns.lock().unwrap().len(), 3);
        let conn = state.db.connect().unwrap();
        let statuses: Vec<(String, String)> = conn
            .prepare("SELECT title, status FROM bot_threads WHERE project_id = 'p1' ORDER BY title")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(statuses.iter().all(|(_, s)| s == "review"), "{statuses:?}");
        // The bot's closing checklist ticks the plan: step 1 done, step 2 not.
        let todo: String = conn.query_row("SELECT todo FROM bot_threads WHERE title = 'Research cloud pricing'", [], |r| r.get(0)).unwrap();
        let todo: Vec<TodoItem> = serde_json::from_str(&todo).unwrap();
        assert_eq!(todo.iter().map(|i| i.state.as_str()).collect::<Vec<_>>(), ["done", "pending"]);
        let team: i64 = conn.query_row("SELECT COUNT(*) FROM project_bots WHERE project_id = 'p1'", [], |r| r.get(0)).unwrap();
        assert_eq!(team, 3);
        let kinds: Vec<String> = conn
            .prepare("SELECT json_extract(payload, '$.kind') FROM project_messages WHERE role = 'coordinator' ORDER BY created_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(kinds.iter().filter(|k| *k == "completed").count(), 3);
        // All back: Al posts one wrap-up, and not again without new results.
        assert_eq!(kinds.last().map(String::as_str), Some("synthesis"));
        assert!(maybe_synthesize(&state.db, &rt, "u", "p1").await.is_none());
        let syntheses: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_messages WHERE json_extract(payload, '$.kind') = 'synthesis'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(syntheses, 1);

        // The plan is on the canonical DAG: one node per thread, the engine
        // blocked by economics, and every node moved READY → RUNNING → DONE.
        assert_eq!(out.reply["payload"]["dagId"], "dag_1");
        let graph = rt.graph.lock().unwrap().clone();
        assert_eq!(graph.len(), 3);
        assert_eq!(graph.iter().find(|g| g.0 == "engine").unwrap().2, vec!["econ".to_string()]);
        let nodes: Vec<(String, String)> = conn
            .prepare("SELECT dag_id, dag_node_id FROM bot_threads WHERE project_id = 'p1'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(nodes.iter().all(|(d, n)| d == "dag_1" && n.starts_with("node-")), "{nodes:?}");
        let moves = rt.moves.lock().unwrap().clone();
        for key in ["research", "econ", "engine"] {
            let node = format!("node-{key}");
            let mine: Vec<(String, String)> = moves.iter().filter(|m| m.0 == node).map(|m| (m.1.clone(), m.2.clone())).collect();
            assert_eq!(mine, vec![("READY".into(), "RUNNING".into()), ("RUNNING".into(), "DONE".into())], "{key}");
        }
    }

    #[tokio::test]
    async fn gizzi_coordinator_writes_the_plan_to_the_rails_dag() {
        let state = setup("dag").await;
        let rt = GizziCoordinator { state: state.clone() };
        let steps = vec![
            ("econ".to_string(), "GPU unit economics".to_string(), vec![]),
            ("engine".to_string(), "Pricing engine".to_string(), vec!["econ".to_string()]),
        ];
        let (dag_id, nodes) = rt.mirror_plan("Price the cloud", "p1", &steps).await.expect("dag");
        let events = state.rails.ledger.query(allternit_commrails::LedgerQuery::default()).await.unwrap();
        let dag = allternit_commrails::work::project_dag(&events, &dag_id);
        assert_eq!(dag.nodes[&nodes["engine"]].title, "Pricing engine");
        assert!(dag
            .edges
            .iter()
            .any(|e| e.from_node_id == nodes["econ"] && e.to_node_id == nodes["engine"] && e.edge_type == "blocked_by"));
        rt.node_status(&dag_id, &nodes["econ"], "READY", "DONE").await;
        let events = state.rails.ledger.query(allternit_commrails::LedgerQuery::default()).await.unwrap();
        assert_eq!(allternit_commrails::work::project_dag(&events, &dag_id).nodes[&nodes["econ"]].status, "DONE");
    }

    #[tokio::test]
    async fn follow_up_routes_to_one_open_thread_and_invalid_plans_fall_back() {
        let state = setup("route").await;
        let rt = Fake::default();
        // Invalid plan (unknown bot) → one fallback thread.
        *rt.plan.lock().unwrap() = Some(json!({"action": "plan", "reply": "x", "steps": [{"key": "a", "title": "Design page", "bot": "Pixel"}]}).to_string());
        let out = coordinate(&state.db, &rt, "u", "p1", "Draft the release notes for Friday").await.unwrap();
        assert_eq!(out.reply["payload"]["planned"], false);
        let tid = out.reply["payload"]["threads"][0]["id"].as_str().unwrap().to_string();

        *rt.plan.lock().unwrap() = Some(json!({"action": "route", "reply": "Sent that to the release notes thread.", "threadId": tid}).to_string());
        let out2 = coordinate(&state.db, &rt, "u", "p1", "The release moved to Monday").await.unwrap();
        assert_eq!(out2.reply["payload"]["kind"], "routed");
        assert_eq!(out2.start.len(), 1);
        assert!(out2.start[0].1.starts_with("[forwarded from project chat] The release moved to Monday"));

        // A route to a thread outside the project is ignored → fallback thread instead.
        *rt.plan.lock().unwrap() = Some(json!({"action": "route", "reply": "x", "threadId": "not-a-thread"}).to_string());
        let out3 = coordinate(&state.db, &rt, "u", "p1", "Another request").await.unwrap();
        assert_eq!(out3.reply["payload"]["kind"], "fanout");
    }

    #[tokio::test]
    async fn planner_runs_on_the_team_bots_model() {
        let state = setup("model").await;
        let team = load_team(&state.db, "p1", "u").unwrap();
        assert_eq!(team_model(&state.db, &team), Some(("p".to_string(), "m".to_string())));
    }

    #[tokio::test]
    async fn failed_kickoff_blocks_the_thread_and_says_why() {
        let state = setup("blocked").await;
        let rt = Fake { fail_turns: true, ..Default::default() };
        let out = coordinate(&state.db, &rt, "u", "p1", "Research GPU prices").await.unwrap();
        start_threads(&state.db, &rt, "u", "p1", out.start).await;
        let conn = state.db.connect().unwrap();
        let (status, line): (String, String) = conn.query_row("SELECT status, status_line FROM bot_threads WHERE project_id = 'p1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(status, "blocked");
        assert!(line.contains("provider_rate_limit"));
    }

    /// A template's plan: two roots (Scout, Ledger) and one dependent (Forge,
    /// waiting on the Ledger thread), created the way the UI creates them.
    async fn template_plan(state: &Arc<AppState>, rt: &Fake) -> (String, String, String) {
        let mk = |bot: &str, title: &str, status: &str, deps: Vec<String>| {
            serde_json::from_value::<CreateThreadBody>(json!({
                "botId": bot, "title": title, "projectId": "p1", "kind": "task", "objective": title,
                "status": status, "dependsOn": deps, "createdBy": "template"
            }))
            .unwrap()
        };
        let a = thread_routes::create(&state.db, rt, "u", mk("scout", "Competitor pricing check", "idle", vec![])).await.unwrap();
        let b = thread_routes::create(&state.db, rt, "u", mk("ledger", "Price from unit costs", "idle", vec![])).await.unwrap();
        let c = thread_routes::create(&state.db, rt, "u", mk("forge", "Wire pricing into checkout", "queued", vec![b.id.clone()])).await.unwrap();
        (a.id, b.id, c.id)
    }

    #[tokio::test]
    async fn start_refuses_other_users_projects_and_threads_outside_the_project() {
        let state = setup("start-owner").await;
        let rt = Fake::default();
        let (a, _, _) = template_plan(&state, &rt).await;
        let conn = state.db.connect().unwrap();
        conn.execute("INSERT INTO cowork_projects (id, user_id, title) VALUES ('p2', 'u', 'Other project')", []).unwrap();
        conn.execute("INSERT INTO cowork_projects (id, user_id, title) VALUES ('px', 'someone-else', 'Not yours')", []).unwrap();

        assert_eq!(accept_start(&state.db, "someone-else", "p1", vec![(a.clone(), "go".into())]), Err(StartRefusal::ProjectNotFound));
        assert_eq!(accept_start(&state.db, "u", "px", vec![(a.clone(), "go".into())]), Err(StartRefusal::ProjectNotFound));
        assert!(matches!(accept_start(&state.db, "u", "p2", vec![(a.clone(), "go".into())]), Err(StartRefusal::BadThread(_))));
        assert!(matches!(accept_start(&state.db, "u", "p1", vec![(a.clone(), "go".into()), ("nope".into(), "go".into())]), Err(StartRefusal::BadThread(_))));
        assert_eq!(accept_start(&state.db, "u", "p1", vec![]), Err(StartRefusal::Empty));
        // Nothing was claimed by a refused request.
        let status: String = conn.query_row("SELECT status FROM bot_threads WHERE id = ?1", params![a], |r| r.get(0)).unwrap();
        assert_eq!(status, "idle");
    }

    #[tokio::test]
    async fn started_template_roots_run_through_the_coordinator() {
        let state = setup("start-run").await;
        let rt = Fake { rendezvous: Some(Arc::new(tokio::sync::Barrier::new(2))), ..Fake::default() };
        let (a, b, c) = template_plan(&state, &rt).await;

        let (start, skipped) = accept_start(&state.db, "u", "p1", vec![(a.clone(), "Check competitor prices".into()), (b.clone(), String::new()), (a.clone(), "dup".into())]).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(start.iter().map(|s| s.0.clone()).collect::<Vec<_>>(), vec![a.clone(), b.clone()], "deduplicated, in order");
        assert_eq!(start[0].1, "Check competitor prices", "the caller's kickoff text is used");
        assert!(start[1].1.contains("Objective: Price from unit costs"), "an empty text gets the coordinator's kickoff");
        // Claimed: working at once, and a second request can't start them again.
        let conn = state.db.connect().unwrap();
        let status = |id: &str| conn.query_row("SELECT status FROM bot_threads WHERE id = ?1", params![id], |r| r.get::<_, String>(0)).unwrap();
        assert_eq!(status(&a), "working");
        let (again, skipped) = accept_start(&state.db, "u", "p1", vec![(a.clone(), "go".into())]).unwrap();
        assert!(again.is_empty());
        assert_eq!(skipped, vec![a.clone()]);

        // The two roots run at the same time (the barrier needs both), then
        // the dependent starts and Al wraps up.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            // The dependent's own turn hits the barrier alone: give it a partner.
            let rt2 = &rt;
            futures::future::join(start_threads(&state.db, rt2, "u", "p1", start), async {
                loop {
                    if status(&c) == "working" {
                        rt2.rendezvous.as_ref().unwrap().wait().await;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
        })
        .await
        .expect("roots must run concurrently");
        assert_eq!(rt.turns.lock().unwrap().len(), 3);
        for id in [&a, &b, &c] {
            assert_eq!(status(id), "review", "{id}");
        }
        let kinds: Vec<String> = conn
            .prepare("SELECT json_extract(payload, '$.kind') FROM project_messages WHERE project_id = 'p1' ORDER BY created_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(kinds.iter().filter(|k| *k == "completed").count(), 3);
    }
}
