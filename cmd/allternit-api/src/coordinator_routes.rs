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

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
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
    Router::new().route("/projects/:project_id/messages", get(list_messages).post(post_message))
}

// ─── Runtime seam ───────────────────────────────────────────────────────────

/// Everything the coordinator needs from the outside world.
pub trait CoordinatorRuntime: ThreadRuntime {
    /// One-shot planning completion on `model` (provider, model) when given,
    /// else the platform default; `None` when the model is unavailable.
    fn plan(&self, system: &str, prompt: &str, model: Option<(String, String)>) -> impl Future<Output = Option<String>> + Send;
    /// Run a turn in a thread's session; returns the bot's reply text.
    fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> impl Future<Output = Result<String, String>> + Send;
}

pub struct GizziCoordinator {
    pub state: Arc<AppState>,
}

impl ThreadRuntime for GizziCoordinator {
    async fn create_session(&self, bot_id: &str, bot_name: &str, title: &str, canonical: bool, thread_id: &str) -> Result<String, String> {
        crate::agent_session_routes::create_bot_thread_session(&self.state.db, bot_id, bot_name, title, canonical, Some(thread_id)).await
    }
    async fn seed(&self, session_id: &str, text: &str) -> Result<(), String> {
        crate::agent_session_routes::seed_session_message(session_id, text).await
    }
}

impl CoordinatorRuntime for GizziCoordinator {
    async fn plan(&self, system: &str, prompt: &str, model: Option<(String, String)>) -> Option<String> {
        crate::gizzi_completion::complete_ephemeral(prompt, Some(system), model.as_ref()).await
    }
    async fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn(&self.state.db, session_id, bot_id, text).await
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
1. A new request: {\"action\":\"plan\",\"reply\":\"<one or two plain sentences to the user>\",\"steps\":[{\"key\":\"<short-id>\",\"title\":\"<3-6 words>\",\"objective\":\"<what done looks like>\",\"bot\":\"<exact bot name from the team>\",\"dependsOn\":[\"<key of a step that must finish first>\"],\"todo\":[\"<2-4 short steps>\"]}]}\n\
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
    let msg = add_message(db, project_id, user_id, "coordinator", &text, json!({"kind": "fanout", "threads": ids, "planned": planned}))
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
    s
}

/// Run kickoff turns; each finished thread moves to review, Al reports it,
/// and any dependents whose inputs are all ready start next.
pub async fn start_threads<R: CoordinatorRuntime>(db: &DbHandle, rt: &R, user_id: &str, project_id: &str, start: Vec<(String, String)>) {
    let mut queue = start;
    while let Some((thread_id, text)) = queue.pop() {
        let Ok(Some(t)) = thread_routes::load_view(db, &thread_id) else { continue };
        let Some(session) = t.current_session_id.clone() else { continue };
        set_status(db, &t.id, "working", Some("Working on it"), None);
        match rt.send_turn(&session, &t.bot_id, &text).await {
            Ok(reply) => {
                let line = reply.lines().find(|l| !l.trim().is_empty()).map(|l| truncate(l.trim(), 140));
                set_status(db, &t.id, "review", line.as_deref(), Some(&truncate(&reply, 1200)));
                let _ = add_message(
                    db,
                    project_id,
                    user_id,
                    "coordinator",
                    &format!("The {} thread has an update ready for you{}", t.title, line.as_ref().map(|l| format!(": {l}")).unwrap_or_else(|| ".".into())),
                    json!({"kind": "completed", "threadId": t.id, "threadTitle": t.title}),
                );
                queue.extend(ready_dependents(db, project_id, &t.id));
            }
            Err(e) => {
                set_status(db, &t.id, "blocked", Some(&truncate(&e, 140)), None);
                let _ = add_message(
                    db,
                    project_id,
                    user_id,
                    "coordinator",
                    &format!("The {} thread is blocked: {}", t.title, truncate(&e, 200)),
                    json!({"kind": "blocked", "threadId": t.id, "threadTitle": t.title}),
                );
            }
        }
    }
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
    }

    impl CoordinatorRuntime for Fake {
        async fn plan(&self, _s: &str, _p: &str, _m: Option<(String, String)>) -> Option<String> {
            self.plan.lock().unwrap().clone()
        }
        async fn send_turn(&self, s: &str, _b: &str, t: &str) -> Result<String, String> {
            if self.fail_turns {
                return Err("provider_rate_limit".into());
            }
            self.turns.lock().unwrap().push((s.into(), t.into()));
            Ok("Found three prices.\nDetails follow.".into())
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
        PlanStep { key: key.into(), title: format!("Step {key}"), objective: String::new(), bot: bot.into(), depends_on: deps.iter().map(|s| s.to_string()).collect(), todo: vec![] }
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
}
