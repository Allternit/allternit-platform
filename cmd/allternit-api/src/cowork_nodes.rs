//! The cowork queue, folded into Factory workspace nodes (SPEC §0.9, §3).
//!
//! There is one list of work. A cowork **task** is a node in its owner's
//! standing Tasks DAG, written only through the Gate and read back as a
//! projection of the ledger. The `/api/v1/tasks` and `/api/v1/queue` routes
//! keep their HTTP shapes for one release, but they read and write nodes here.
//!
//! | Cowork            | Factory                                                    |
//! |-------------------|------------------------------------------------------------|
//! | task              | node `task-<taskId>` in DAG `tasks-<user>-<workspace>`     |
//! | task status       | node status (+ the `column` state for backlog vs todo)     |
//! | task assignee     | node `assignee` (`<agentId>` or `user:<id>`)               |
//! | queue item        | a claim on that node (`queue_*` state), one per node       |
//! | queue claim/start | READY → IN_PROGRESS through the Gate (first claimer wins)  |
//! | queue complete    | IN_PROGRESS → VERIFYING ("in-review"): the owner closes it |
//! | run / job         | already DAG + nodes (`rails_client_impl::LocalRailsClient`) |
//!
//! A completed queue item never marks its task done: the worker's word is
//! narrative, and proof comes from the judge, receipts, or the owner closing
//! the task (determinism contract rule 9). A failed item goes back to the
//! queue until `max_retries` (rule 10), then the node fails.
//!
//! The `tasks` table stays as a read model for comments, audit logs and
//! older readers. It is rewritten from the node after every Gate write.
//! Rows that never became nodes (done/closed history from before the fold)
//! stay readable from it, and are folded the first time they're edited.
//! [`fold_legacy`] is the one-time fold of every open task and queue row
//! (`--dry-run` first, counts reported, idempotent).

use std::collections::HashMap;
use std::sync::Arc;

use allternit_factory_engine::core::types::{AllternitEvent, LedgerQuery};
use allternit_factory_engine::gate::gate::DagMutation;
use allternit_factory_engine::kernel::lifecycle::check_legacy_change;
use allternit_factory_engine::work::{project_dag, DagNode};
use allternit_factory_engine::workspace::board;
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::rails::RailsState;
use crate::task_routes::Task;

/// Every cowork write runs one at a time in this process, so the Gate's
/// `from` check on a status change is a real compare-and-swap (two workers
/// can't both claim the same queue item).
static WRITE_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

pub const NODE_KIND: &str = "task";
pub const TASK_LABEL: &str = "cowork:task";
const DEFAULT_MAX_RETRIES: i64 = 3;

// ─── IDs ────────────────────────────────────────────────────────────────────

fn short_hash(s: &str, n: usize) -> String {
    let digest = Sha256::digest(s.as_bytes());
    hex::encode(digest)[..n].to_string()
}

/// The prefix of every Tasks DAG a user owns.
pub fn user_prefix(user_id: &str) -> String {
    format!("tasks-{}-", short_hash(user_id, 16))
}

/// The standing Tasks DAG for (user, workspace). An empty workspace is the
/// user's personal list.
pub fn dag_id(user_id: &str, workspace_id: &str) -> String {
    let ws = if workspace_id.trim().is_empty() { "personal".to_string() } else { short_hash(workspace_id.trim(), 12) };
    format!("{}{}", user_prefix(user_id), ws)
}

pub fn node_id(task_id: &str) -> String {
    format!("task-{task_id}")
}

/// Task ids become node ids and folder names, so they are kept to a safe set.
pub fn valid_task_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// A failure in the contract's words (API.md §2 codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoworkError {
    pub code: &'static str,
    pub fact: String,
    pub action: String,
}

impl CoworkError {
    fn new(code: &'static str, fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self { code, fact: fact.into(), action: action.into() }
    }
    pub fn not_found(fact: impl Into<String>) -> Self {
        Self::new("not_found", fact, "List tasks with GET /api/v1/tasks.")
    }
    pub fn usage(fact: impl Into<String>, action: impl Into<String>) -> Self {
        Self::new("usage", fact, action)
    }
    fn from_gate(e: anyhow::Error) -> Self {
        if allternit_factory_engine::GateError::from_anyhow(&e).is_some() {
            Self::new("refused", format!("{e:#}"), "Reload the task and try again.")
        } else {
            Self::new("internal", format!("{e:#}"), "Retry. If it keeps failing, check the Factory ledger in the API data dir.")
        }
    }
    pub fn http_status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self.code {
            "not_found" => StatusCode::NOT_FOUND,
            "usage" => StatusCode::BAD_REQUEST,
            "refused" => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    /// `{ error: { code, fact, action } }`, plus the older `message` string
    /// the cowork clients print.
    pub fn body(&self) -> Value {
        json!({ "error": { "code": self.code, "fact": self.fact, "action": self.action }, "message": self.fact })
    }
}

impl std::fmt::Display for CoworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.fact)
    }
}

pub type CoworkResult<T> = std::result::Result<T, CoworkError>;

// ─── Status mapping ─────────────────────────────────────────────────────────

/// A task status the cowork routes accept, normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
    Cancelled,
    Failed,
}

impl TaskStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "backlog" => Self::Backlog,
            "todo" | "to-do" | "pending" | "open" | "ready" => Self::Todo,
            "in-progress" | "inprogress" | "doing" | "running" | "active" => Self::InProgress,
            "in-review" | "review" | "inreview" | "checking" => Self::InReview,
            "done" | "completed" | "complete" | "closed" => Self::Done,
            "cancelled" | "canceled" => Self::Cancelled,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in-progress",
            Self::InReview => "in-review",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::Failed)
    }
}

const ALLOWED_STATUSES: &str = "backlog, todo, in-progress, in-review, done, cancelled or failed";

fn parse_status(raw: &str) -> CoworkResult<TaskStatus> {
    TaskStatus::parse(raw).ok_or_else(|| {
        CoworkError::usage(format!("unknown task status {raw:?}"), format!("Use one of {ALLOWED_STATUSES}."))
    })
}

fn node_closed(status: &str) -> bool {
    matches!(status, "DONE" | "PASS" | "COMPLETED" | "FAILED" | "FAIL" | "CANCELLED")
}

/// The node status a task status lands on, given where the node is now.
/// Backlog is NEW only for a node that hasn't started (or is reopened);
/// a live node can't go back to NEW, so it waits at READY in the backlog
/// column instead.
fn target_node_status(status: TaskStatus, current: &str) -> &'static str {
    match status {
        TaskStatus::Backlog if current == "NEW" || node_closed(current) => "NEW",
        TaskStatus::Backlog | TaskStatus::Todo => "READY",
        TaskStatus::InProgress => "IN_PROGRESS",
        TaskStatus::InReview => "VERIFYING",
        TaskStatus::Done => "DONE",
        TaskStatus::Cancelled => "CANCELLED",
        TaskStatus::Failed => "FAILED",
    }
}

/// The Gate status changes from `current` to `target`: none, one, or a
/// reopen (closed → NEW) followed by the move.
fn status_changes(node: &str, current: &str, target: &str) -> CoworkResult<Vec<DagMutation>> {
    let change = |from: &str, to: &str| DagMutation::ChangeStatus {
        node_id: node.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        reason: Some("cowork task status".to_string()),
    };
    if current == target {
        return Ok(vec![]);
    }
    if check_legacy_change(current, target).is_ok() {
        return Ok(vec![change(current, target)]);
    }
    if node_closed(current) && check_legacy_change("NEW", target).is_ok() {
        let mut out = vec![change(current, "NEW")];
        if target != "NEW" {
            out.push(change("NEW", target));
        }
        return Ok(out);
    }
    Err(CoworkError::new(
        "refused",
        format!("a task can't move from node status {current} to {target}"),
        "Move it to todo first.",
    ))
}

/// The task status a node shows.
pub fn task_status_of(node: &DagNode) -> TaskStatus {
    match node.status.as_str() {
        "DONE" | "PASS" | "COMPLETED" => TaskStatus::Done,
        "FAILED" | "FAIL" => TaskStatus::Failed,
        "CANCELLED" => TaskStatus::Cancelled,
        "VERIFYING" | "OUTPUT_READY" | "NEEDS_HUMAN" => TaskStatus::InReview,
        "IN_PROGRESS" | "RUNNING" | "LEASED" | "SPAWNED" | "EXCEPTION" | "CONTINUE" => TaskStatus::InProgress,
        _ => match node.state.get("column").map(String::as_str) {
            Some("backlog") => TaskStatus::Backlog,
            Some("todo") => TaskStatus::Todo,
            _ if node.status == "NEW" => TaskStatus::Backlog,
            _ => TaskStatus::Todo,
        },
    }
}

// ─── Reading ────────────────────────────────────────────────────────────────

pub async fn ledger_events(rails: &RailsState) -> CoworkResult<Vec<AllternitEvent>> {
    rails
        .ledger
        .query(LedgerQuery::default())
        .await
        .map_err(|e| CoworkError::new("internal", format!("reading the Factory ledger failed: {e:#}"), "Check the ledger under the API data dir."))
}

/// The user's Tasks DAG ids, in ledger order.
pub fn user_dags(events: &[AllternitEvent], user_id: &str) -> Vec<String> {
    let prefix = user_prefix(user_id);
    board::dag_ids(events).into_iter().filter(|d| d.starts_with(&prefix)).collect()
}

/// Task nodes of one DAG that belong to `user_id`.
fn task_nodes(events: &[AllternitEvent], dag: &str, user_id: &str) -> Vec<DagNode> {
    let state = project_dag(&board::dag_events(events, dag), dag);
    state
        .nodes
        .into_values()
        .filter(|n| n.node_kind == NODE_KIND && n.state.get("user_id").map(String::as_str) == Some(user_id))
        .collect()
}

/// Every task node the user owns, optionally in one workspace.
pub fn user_task_nodes(events: &[AllternitEvent], user_id: &str, workspace_id: Option<&str>) -> Vec<(String, DagNode)> {
    let dags = match workspace_id {
        Some(ws) => {
            let d = dag_id(user_id, ws);
            user_dags(events, user_id).into_iter().filter(|x| *x == d).collect()
        }
        None => user_dags(events, user_id),
    };
    dags.into_iter()
        .flat_map(|d| task_nodes(events, &d, user_id).into_iter().map(move |n| (d.clone(), n)))
        .collect()
}

/// The node for `task_id`, when the user owns it.
pub fn find(events: &[AllternitEvent], user_id: &str, task_id: &str) -> Option<(String, DagNode)> {
    let want = node_id(task_id);
    user_dags(events, user_id).into_iter().find_map(|d| {
        task_nodes(events, &d, user_id).into_iter().find(|n| n.node_id == want).map(|n| (d, n))
    })
}

fn st<'a>(node: &'a DagNode, key: &str) -> Option<&'a str> {
    node.state.get(key).map(String::as_str).filter(|s| !s.is_empty())
}

/// A node, in the `Task` shape the cowork clients already read.
pub fn task_of(dag: &str, node: &DagNode) -> Task {
    let task_id = st(node, "task_id").map(str::to_string).unwrap_or_else(|| node.node_id.trim_start_matches("task-").to_string());
    Task {
        id: task_id,
        user_id: st(node, "user_id").unwrap_or_default().to_string(),
        workspace_id: Some(st(node, "workspace_id").unwrap_or_default().to_string()),
        title: node.title.clone(),
        description: Some(node.description.clone().unwrap_or_default()),
        status: task_status_of(node).as_str().to_string(),
        priority: st(node, "priority").map(str::to_string).or_else(|| node.priority.map(|p| p.to_string())).unwrap_or_else(|| "50".to_string()),
        assignee_id: Some(st(node, "assignee_id").unwrap_or_default().to_string()),
        due_date: Some(st(node, "due_date").unwrap_or_default().to_string()),
        tags: Some(st(node, "tags").unwrap_or_default().to_string()),
        metadata: Some(st(node, "metadata").unwrap_or_default().to_string()),
        created_at: st(node, "created_at").map(str::to_string).or_else(|| node.created_at.clone()).unwrap_or_default(),
        updated_at: node.updated_at.clone().or_else(|| node.created_at.clone()).unwrap_or_default(),
        assignee_type: Some(st(node, "assignee_type").unwrap_or_default().to_string()),
        assignee_name: Some(st(node, "assignee_name").unwrap_or_default().to_string()),
        dag_id: Some(dag.to_string()),
        node_id: Some(node.node_id.clone()),
    }
}

// ─── Writing ────────────────────────────────────────────────────────────────

/// Task fields from a create/update/assign body. `None` leaves a field as it
/// is; `Some("")` clears it.
#[derive(Debug, Clone, Default)]
pub struct TaskFields {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<String>,
    pub assignee_type: Option<String>,
    pub assignee_name: Option<String>,
    pub due_date: Option<String>,
    pub tags: Option<String>,
    pub metadata: Option<String>,
}

fn set_state(node: &str, dim: &str, value: &str) -> DagMutation {
    DagMutation::SetState { node_id: node.to_string(), dimension: dim.to_string(), value: value.to_string(), reason: None }
}

/// The node's Factory assignee: an agent id as-is (the bot's address), a
/// person as `user:<id>`, nothing when unassigned.
fn factory_assignee(assignee_type: &str, assignee_id: &str) -> Option<String> {
    let id = assignee_id.trim();
    if id.is_empty() {
        return None;
    }
    Some(if assignee_type == "agent" { id.to_string() } else { format!("user:{id}") })
}

/// The mutations that bring `node` (or a new node) to `fields`.
fn field_mutations(node_id: &str, current: Option<&DagNode>, f: &TaskFields) -> CoworkResult<Vec<DagMutation>> {
    let mut muts = Vec::new();
    let mut patch = serde_json::Map::new();
    if let Some(t) = &f.title {
        if t.trim().is_empty() {
            return Err(CoworkError::usage("a task needs a title", "Send a non-empty title."));
        }
        if current.is_some() {
            patch.insert("title".into(), json!(t));
        }
    }
    if let (Some(d), Some(_)) = (&f.description, current) {
        patch.insert("description".into(), json!(d));
    }
    if let Some(p) = &f.priority {
        if let Ok(n) = p.trim().parse::<i64>() {
            patch.insert("priority".into(), json!(n));
        }
        muts.push(set_state(node_id, "priority", p));
    }
    for (dim, v) in [
        ("due_date", &f.due_date),
        ("tags", &f.tags),
        ("metadata", &f.metadata),
        ("assignee_type", &f.assignee_type),
        ("assignee_id", &f.assignee_id),
        ("assignee_name", &f.assignee_name),
    ] {
        if let Some(v) = v {
            muts.push(set_state(node_id, dim, v));
        }
    }
    if f.assignee_id.is_some() || f.assignee_type.is_some() {
        let kind = f.assignee_type.clone().or_else(|| current.and_then(|n| st(n, "assignee_type").map(str::to_string))).unwrap_or_default();
        let id = f.assignee_id.clone().or_else(|| current.and_then(|n| st(n, "assignee_id").map(str::to_string))).unwrap_or_default();
        patch.insert("assignee".into(), factory_assignee(&kind, &id).map(Value::String).unwrap_or(Value::Null));
    }
    if !patch.is_empty() {
        muts.insert(0, DagMutation::UpdateNode { node_id: node_id.to_string(), patch: Value::Object(patch) });
    }
    Ok(muts)
}

fn status_mutations(node_id: &str, current_status: &str, status: TaskStatus) -> CoworkResult<Vec<DagMutation>> {
    let target = target_node_status(status, current_status);
    let mut muts = status_changes(node_id, current_status, target)?;
    if matches!(status, TaskStatus::Backlog | TaskStatus::Todo) {
        muts.push(set_state(node_id, "column", status.as_str()));
    }
    Ok(muts)
}

/// What a write would do (`dryRun`), or did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WritePlan {
    pub dag_id: String,
    pub node_id: String,
    pub records: Vec<String>,
}

fn describe(m: &DagMutation) -> String {
    match m {
        DagMutation::CreateNode { node_id, title, .. } => format!("create node {node_id} \"{title}\""),
        DagMutation::UpdateNode { node_id, patch } => format!("update node {node_id} {patch}"),
        DagMutation::DeleteNode { node_id } => format!("delete node {node_id}"),
        DagMutation::ChangeStatus { node_id, from, to, .. } => format!("node {node_id} status {from} -> {to}"),
        DagMutation::SetState { node_id, dimension, value, .. } => format!("node {node_id} {dimension} = {value:?}"),
        DagMutation::AddLabel { node_id, label } => format!("node {node_id} label {label}"),
        other => format!("{other:?}"),
    }
}

async fn apply(rails: &RailsState, dag: &str, note: &str, muts: Vec<DagMutation>) -> CoworkResult<()> {
    if muts.is_empty() {
        return Ok(());
    }
    rails.gate.mutate_with_decision(dag, note, Some("cowork".to_string()), muts).await.map(|_| ()).map_err(CoworkError::from_gate)
}

async fn ensure_dag(rails: &RailsState, dag: &str, workspace_id: &str) -> CoworkResult<()> {
    let intent = if workspace_id.trim().is_empty() { "Tasks".to_string() } else { format!("Tasks ({})", workspace_id.trim()) };
    rails.gate.ensure_dag(dag, &intent).await.map(|_| ()).map_err(CoworkError::from_gate)
}

/// The mutations that create a task node with `fields` (status defaults to todo).
fn create_mutations(user_id: &str, workspace_id: &str, task_id: &str, created_at: Option<&str>, f: &TaskFields) -> CoworkResult<Vec<DagMutation>> {
    let title = f.title.clone().unwrap_or_default();
    if title.trim().is_empty() {
        return Err(CoworkError::usage("a task needs a title", "Send a non-empty title."));
    }
    let status = parse_status(f.status.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("todo"))?;
    let node = node_id(task_id);
    let mut muts = vec![
        DagMutation::CreateNode {
            node_id: node.clone(),
            node_kind: NODE_KIND.to_string(),
            title,
            parent_node_id: None,
            execution_mode: "shared".to_string(),
            description: f.description.clone().filter(|d| !d.is_empty()),
            executor: None,
        },
        DagMutation::AddLabel { node_id: node.clone(), label: TASK_LABEL.to_string() },
        // Factory approvals for this node go to its owner.
        DagMutation::AddLabel { node_id: node.clone(), label: format!("owner:{user_id}") },
        set_state(&node, "user_id", user_id),
        set_state(&node, "workspace_id", workspace_id),
        set_state(&node, "task_id", task_id),
        set_state(&node, "created_at", &created_at.map(str::to_string).unwrap_or_else(|| chrono::Utc::now().to_rfc3339())),
    ];
    let mut rest = TaskFields { title: None, description: None, status: None, ..f.clone() };
    if rest.priority.is_none() {
        rest.priority = Some("50".to_string());
    }
    muts.extend(field_mutations(&node, None, &rest)?);
    muts.extend(status_mutations(&node, "NEW", status)?);
    Ok(muts)
}

pub enum Created {
    /// The plan only (`dryRun`).
    Planned(WritePlan),
    /// A new node; `true` when it was created now, `false` when the id
    /// already named this user's task (idempotent create).
    Task(Task, bool),
}

/// Create a task node. A repeated id returns the existing task.
pub async fn create(rails: &RailsState, user_id: &str, workspace_id: &str, task_id: &str, f: &TaskFields, dry_run: bool) -> CoworkResult<Created> {
    if !valid_task_id(task_id) {
        return Err(CoworkError::usage(format!("task id {task_id:?} isn't valid"), "Use letters, digits, '-', '_', '.' or ':' (at most 128)."));
    }
    let _guard = WRITE_LOCK.lock().await;
    let events = ledger_events(rails).await?;
    if let Some((dag, node)) = find(&events, user_id, task_id) {
        return Ok(Created::Task(task_of(&dag, &node), false));
    }
    if node_exists_anywhere(&events, &node_id(task_id)) {
        return Err(CoworkError::new("refused", format!("task id {task_id:?} is taken"), "Create it without an id, or with a different one."));
    }
    let dag = dag_id(user_id, workspace_id);
    let muts = create_mutations(user_id, workspace_id, task_id, None, f)?;
    if dry_run {
        let mut records = vec![format!("ensure DAG {dag}")];
        records.extend(muts.iter().map(describe));
        return Ok(Created::Planned(WritePlan { dag_id: dag, node_id: node_id(task_id), records }));
    }
    ensure_dag(rails, &dag, workspace_id).await?;
    apply(rails, &dag, "cowork task created", muts).await?;
    let events = ledger_events(rails).await?;
    let (dag, node) = find(&events, user_id, task_id).ok_or_else(|| CoworkError::new("internal", "the task node wasn't found after it was written", "Retry."))?;
    Ok(Created::Task(task_of(&dag, &node), true))
}

fn node_exists_anywhere(events: &[AllternitEvent], node: &str) -> bool {
    events.iter().any(|e| e.r#type == "DagNodeCreated" && e.payload.get("node_id").and_then(|v| v.as_str()) == Some(node))
}

/// Update a task node. Returns the plan with `dry_run`, else the task.
pub async fn update(rails: &RailsState, user_id: &str, task_id: &str, f: &TaskFields, dry_run: bool) -> CoworkResult<std::result::Result<WritePlan, Task>> {
    let _guard = WRITE_LOCK.lock().await;
    let events = ledger_events(rails).await?;
    let (dag, node) = find(&events, user_id, task_id).ok_or_else(|| CoworkError::not_found(format!("task {task_id} not found")))?;
    let mut muts = field_mutations(&node.node_id, Some(&node), f)?;
    if let Some(raw) = f.status.as_deref().filter(|s| !s.trim().is_empty()) {
        muts.extend(status_mutations(&node.node_id, &node.status, parse_status(raw)?)?);
    }
    if dry_run {
        return Ok(Ok(WritePlan { dag_id: dag, node_id: node.node_id, records: muts.iter().map(describe).collect() }));
    }
    apply(rails, &dag, "cowork task updated", muts).await?;
    let events = ledger_events(rails).await?;
    let (dag, node) = find(&events, user_id, task_id).ok_or_else(|| CoworkError::not_found(format!("task {task_id} not found")))?;
    Ok(Err(task_of(&dag, &node)))
}

/// Delete a task node. The ledger keeps its history.
pub async fn delete(rails: &RailsState, user_id: &str, task_id: &str) -> CoworkResult<()> {
    let _guard = WRITE_LOCK.lock().await;
    let events = ledger_events(rails).await?;
    let (dag, node) = find(&events, user_id, task_id).ok_or_else(|| CoworkError::not_found(format!("task {task_id} not found")))?;
    apply(rails, &dag, "cowork task deleted", vec![DagMutation::DeleteNode { node_id: node.node_id }]).await
}

// ─── The read model (`tasks` table) ─────────────────────────────────────────

/// Rewrite the task's `tasks` row from its node (comments, audit logs and
/// older readers join on it).
pub fn write_row(conn: &rusqlite::Connection, t: &Task) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO tasks (id, user_id, workspace_id, title, description, status, priority, assignee_id, due_date, tags, metadata, assignee_type, assignee_name, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET workspace_id = excluded.workspace_id, title = excluded.title, description = excluded.description,
           status = excluded.status, priority = excluded.priority, assignee_id = excluded.assignee_id, due_date = excluded.due_date,
           tags = excluded.tags, metadata = excluded.metadata, assignee_type = excluded.assignee_type, assignee_name = excluded.assignee_name,
           updated_at = CURRENT_TIMESTAMP
         WHERE tasks.user_id = excluded.user_id",
        rusqlite::params![
            t.id, t.user_id, t.workspace_id, t.title, t.description, t.status, t.priority, t.assignee_id, t.due_date, t.tags, t.metadata,
            t.assignee_type, t.assignee_name
        ],
    )?;
    Ok(())
}

/// A `tasks` row from before the fold, in the task shape.
pub fn legacy_row(conn: &rusqlite::Connection, task_id: &str) -> rusqlite::Result<Option<Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, workspace_id, title, description, status, priority, assignee_id, due_date, tags, metadata,
                created_at, updated_at, assignee_type, assignee_name FROM tasks WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([task_id], crate::task_routes::row_to_task)?;
    rows.next().transpose()
}

/// The user's `tasks` rows that aren't nodes: done/closed history from
/// before the fold (and any open row the fold hasn't reached yet).
pub fn legacy_rows(conn: &rusqlite::Connection, events: &[AllternitEvent], user_id: &str, workspace_id: Option<&str>) -> rusqlite::Result<Vec<Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, workspace_id, title, description, status, priority, assignee_id, due_date, tags, metadata,
                created_at, updated_at, assignee_type, assignee_name FROM tasks WHERE user_id = ?1",
    )?;
    let rows = stmt.query_map([user_id], crate::task_routes::row_to_task)?.filter_map(|r| r.ok()).collect::<Vec<_>>();
    let nodes: std::collections::HashSet<String> = user_task_nodes(events, user_id, None).into_iter().map(|(_, n)| n.node_id).collect();
    Ok(rows
        .into_iter()
        .filter(|t| !nodes.contains(&node_id(&t.id)))
        .filter(|t| workspace_id.map_or(true, |ws| t.workspace_id.as_deref().unwrap_or("") == ws))
        .collect())
}

fn row_fields(t: &Task, status: TaskStatus) -> TaskFields {
    let opt = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
    TaskFields {
        title: Some(t.title.clone()),
        description: opt(&t.description),
        status: Some(status.as_str().to_string()),
        priority: Some(t.priority.clone()).filter(|s| !s.is_empty()),
        assignee_id: opt(&t.assignee_id),
        assignee_type: opt(&t.assignee_type),
        assignee_name: opt(&t.assignee_name),
        due_date: opt(&t.due_date),
        tags: opt(&t.tags),
        metadata: opt(&t.metadata),
    }
}

/// Fold one legacy row into a node (used when an unfolded task is edited).
/// Unknown legacy statuses land on todo.
pub async fn fold_row(rails: &RailsState, t: &Task) -> CoworkResult<()> {
    let _guard = WRITE_LOCK.lock().await;
    fold_row_locked(rails, t).await
}

async fn fold_row_locked(rails: &RailsState, t: &Task) -> CoworkResult<()> {
    if !valid_task_id(&t.id) {
        return Err(CoworkError::usage(format!("legacy task id {:?} can't be a node id", t.id), "Recreate the task."));
    }
    let events = ledger_events(rails).await?;
    if find(&events, &t.user_id, &t.id).is_some() {
        return Ok(());
    }
    let ws = t.workspace_id.clone().unwrap_or_default();
    let dag = dag_id(&t.user_id, &ws);
    let status = TaskStatus::parse(&t.status).unwrap_or(TaskStatus::Todo);
    let created = Some(t.created_at.as_str()).filter(|s| !s.is_empty());
    let muts = create_mutations(&t.user_id, &ws, &t.id, created, &row_fields(t, status))?;
    ensure_dag(rails, &dag, &ws).await?;
    apply(rails, &dag, "cowork task folded into a node", muts).await
}

// ─── The queue ──────────────────────────────────────────────────────────────

/// A queue item, in the shape `/api/v1/queue` has always returned.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QueueItem {
    pub id: String,
    pub task_id: String,
    pub agent_id: Option<String>,
    pub agent_role: Option<String>,
    pub status: String,
    pub claimed_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub retry_count: i64,
    pub max_retries: i64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

fn queue_of(dag: &str, node: &DagNode) -> Option<QueueItem> {
    let id = st(node, "queue_id")?.to_string();
    let s = |k: &str| st(node, k).map(str::to_string);
    let n = |k: &str, d: i64| st(node, k).and_then(|v| v.parse().ok()).unwrap_or(d);
    Some(QueueItem {
        id,
        task_id: s("task_id").unwrap_or_default(),
        agent_id: s("queue_agent_id"),
        agent_role: s("queue_agent_role"),
        status: s("queue_status").unwrap_or_else(|| "pending".to_string()),
        claimed_at: s("queue_claimed_at"),
        started_at: s("queue_started_at"),
        completed_at: s("queue_completed_at"),
        result: s("queue_result"),
        error: s("queue_error"),
        retry_count: n("queue_retry_count", 0),
        max_retries: n("queue_max_retries", DEFAULT_MAX_RETRIES),
        created_at: s("queue_created_at").unwrap_or_default(),
        dag_id: Some(dag.to_string()),
        node_id: Some(node.node_id.clone()),
    })
}

fn queue_open(status: &str) -> bool {
    matches!(status, "pending" | "claimed" | "running")
}

/// The user's queue items (node-backed), newest first.
pub fn list_queue(events: &[AllternitEvent], user_id: &str, workspace_id: Option<&str>, status: Option<&str>) -> Vec<QueueItem> {
    let mut items: Vec<QueueItem> = user_task_nodes(events, user_id, workspace_id)
        .iter()
        .filter_map(|(d, n)| queue_of(d, n))
        .filter(|q| status.map_or(true, |s| q.status == s))
        .collect();
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items
}

fn find_queue(events: &[AllternitEvent], user_id: &str, queue_id: &str) -> Option<(String, DagNode, QueueItem)> {
    user_task_nodes(events, user_id, None).into_iter().find_map(|(d, n)| {
        let q = queue_of(&d, &n)?;
        (q.id == queue_id).then_some((d, n, q))
    })
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Enqueue a task: one open queue item per node. Folds a legacy task first.
pub async fn enqueue(rails: &RailsState, conn: &rusqlite::Connection, user_id: &str, task_id: &str, queue_id: &str, agent_id: Option<&str>, agent_role: Option<&str>) -> CoworkResult<QueueItem> {
    let _guard = WRITE_LOCK.lock().await;
    let mut events = ledger_events(rails).await?;
    if find(&events, user_id, task_id).is_none() {
        match legacy_row(conn, task_id) {
            Ok(Some(t)) if t.user_id == user_id => {
                fold_row_locked(rails, &t).await?;
                events = ledger_events(rails).await?;
            }
            _ => return Err(CoworkError::not_found(format!("task {task_id} not found"))),
        }
    }
    let (dag, node) = find(&events, user_id, task_id).ok_or_else(|| CoworkError::not_found(format!("task {task_id} not found")))?;
    if let Some(q) = queue_of(&dag, &node) {
        if q.id == queue_id {
            return Ok(q);
        }
        if queue_open(&q.status) {
            return Err(CoworkError::new("refused", format!("task {task_id} is already queued as {}", q.id), "Wait for that item to finish, or complete it."));
        }
    }
    if node_closed(&node.status) {
        return Err(CoworkError::new("refused", format!("task {task_id} is closed ({})", task_status_of(&node).as_str()), "Move it back to todo first."));
    }
    let nid = node.node_id.clone();
    let mut muts = status_changes(&nid, &node.status, "READY")?;
    for (k, v) in [
        ("queue_id", queue_id),
        ("queue_status", "pending"),
        ("queue_agent_id", agent_id.unwrap_or("")),
        ("queue_agent_role", agent_role.unwrap_or("")),
        ("queue_retry_count", "0"),
        ("queue_max_retries", &DEFAULT_MAX_RETRIES.to_string()),
        ("queue_created_at", &now()),
        ("queue_claimed_at", ""),
        ("queue_started_at", ""),
        ("queue_completed_at", ""),
        ("queue_result", ""),
        ("queue_error", ""),
    ] {
        muts.push(set_state(&nid, k, v));
    }
    apply(rails, &dag, "cowork task queued", muts).await?;
    let events = ledger_events(rails).await?;
    find_queue(&events, user_id, queue_id).map(|(_, _, q)| q).ok_or_else(|| CoworkError::new("internal", "the queue item wasn't found after it was written", "Retry."))
}

/// Claim the oldest pending item (optionally in one workspace): its node
/// goes READY → IN_PROGRESS and is assigned to the agent. `None` when
/// nothing is pending.
pub async fn claim(rails: &RailsState, user_id: &str, agent_id: &str, agent_role: Option<&str>, workspace_id: Option<&str>) -> CoworkResult<Option<QueueItem>> {
    let _guard = WRITE_LOCK.lock().await;
    let events = ledger_events(rails).await?;
    let mut pending: Vec<(String, DagNode, QueueItem)> = user_task_nodes(&events, user_id, workspace_id)
        .into_iter()
        .filter_map(|(d, n)| queue_of(&d, &n).map(|q| (d, n, q)))
        .filter(|(_, n, q)| q.status == "pending" && n.status == "READY")
        .collect();
    pending.sort_by(|a, b| a.2.created_at.cmp(&b.2.created_at));
    let Some((dag, node, q)) = pending.into_iter().next() else {
        return Ok(None);
    };
    let nid = node.node_id.clone();
    let mut muts = status_changes(&nid, "READY", "IN_PROGRESS")?;
    muts.push(DagMutation::UpdateNode { node_id: nid.clone(), patch: json!({ "assignee": agent_id }) });
    muts.push(set_state(&nid, "queue_status", "claimed"));
    muts.push(set_state(&nid, "queue_agent_id", agent_id));
    if let Some(r) = agent_role {
        muts.push(set_state(&nid, "queue_agent_role", r));
    }
    muts.push(set_state(&nid, "queue_claimed_at", &now()));
    apply(rails, &dag, "cowork queue item claimed", muts).await?;
    let events = ledger_events(rails).await?;
    Ok(find_queue(&events, user_id, &q.id).map(|(_, _, q)| q))
}

pub async fn start(rails: &RailsState, user_id: &str, queue_id: &str) -> CoworkResult<QueueItem> {
    let _guard = WRITE_LOCK.lock().await;
    let events = ledger_events(rails).await?;
    let (dag, node, q) = find_queue(&events, user_id, queue_id).ok_or_else(|| CoworkError::not_found(format!("queue item {queue_id} not found")))?;
    if q.status == "running" {
        return Ok(q);
    }
    if q.status != "claimed" {
        return Err(CoworkError::new("refused", format!("queue item {queue_id} is {}, not claimed", q.status), "Claim it first (POST /api/v1/queue/claim)."));
    }
    let nid = node.node_id.clone();
    let mut muts = status_changes(&nid, &node.status, "IN_PROGRESS")?;
    muts.push(set_state(&nid, "queue_status", "running"));
    muts.push(set_state(&nid, "queue_started_at", &now()));
    apply(rails, &dag, "cowork queue item started", muts).await?;
    let events = ledger_events(rails).await?;
    find_queue(&events, user_id, queue_id).map(|(_, _, q)| q).ok_or_else(|| CoworkError::not_found(format!("queue item {queue_id} not found")))
}

/// Finish a claim. Success puts the task in review (VERIFYING), never done.
/// An error re-queues the task until `max_retries`, then fails the node.
pub async fn complete(rails: &RailsState, user_id: &str, queue_id: &str, result: Option<&str>, error: Option<&str>) -> CoworkResult<QueueItem> {
    let _guard = WRITE_LOCK.lock().await;
    let events = ledger_events(rails).await?;
    let (dag, node, q) = find_queue(&events, user_id, queue_id).ok_or_else(|| CoworkError::not_found(format!("queue item {queue_id} not found")))?;
    if !matches!(q.status.as_str(), "claimed" | "running") {
        return Err(CoworkError::new("refused", format!("queue item {queue_id} is {}, not claimed or running", q.status), "Nothing to complete."));
    }
    let nid = node.node_id.clone();
    let error = error.filter(|e| !e.trim().is_empty());
    let mut muts = Vec::new();
    match error {
        Some(err) => {
            let retries = q.retry_count + 1;
            muts.push(set_state(&nid, "queue_retry_count", &retries.to_string()));
            muts.push(set_state(&nid, "queue_error", err));
            if retries < q.max_retries {
                muts.extend(status_changes(&nid, &node.status, "READY")?);
                muts.push(set_state(&nid, "queue_status", "pending"));
            } else {
                muts.extend(status_changes(&nid, &node.status, "FAILED")?);
                muts.push(set_state(&nid, "queue_status", "failed"));
                muts.push(set_state(&nid, "queue_completed_at", &now()));
            }
        }
        None => {
            muts.extend(status_changes(&nid, &node.status, "VERIFYING")?);
            muts.push(set_state(&nid, "queue_status", "completed"));
            muts.push(set_state(&nid, "queue_result", result.unwrap_or("")));
            muts.push(set_state(&nid, "queue_completed_at", &now()));
        }
    }
    apply(rails, &dag, "cowork queue item completed", muts).await?;
    let events = ledger_events(rails).await?;
    find_queue(&events, user_id, queue_id).map(|(_, _, q)| q).ok_or_else(|| CoworkError::not_found(format!("queue item {queue_id} not found")))
}

/// The task for a node, after a queue write (to refresh the read model).
pub async fn task_for_queue(rails: &RailsState, user_id: &str, queue_id: &str) -> Option<Task> {
    let events = ledger_events(rails).await.ok()?;
    find_queue(&events, user_id, queue_id).map(|(d, n, _)| task_of(&d, &n))
}

/// Closed `cowork_queue` rows from before the fold, readable as history.
pub fn legacy_queue_rows(conn: &rusqlite::Connection, user_id: &str, workspace_id: Option<&str>, status: Option<&str>) -> rusqlite::Result<Vec<QueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT q.id, q.task_id, q.agent_id, q.agent_role, q.status, q.claimed_at, q.started_at, q.completed_at, q.result, q.error,
                COALESCE(q.retry_count, 0), COALESCE(q.max_retries, 3), COALESCE(q.created_at, ''), COALESCE(t.workspace_id, '')
         FROM cowork_queue q JOIN tasks t ON t.id = q.task_id WHERE t.user_id = ?1 AND q.status NOT IN ('pending', 'claimed', 'running')",
    )?;
    let rows = stmt
        .query_map([user_id], |r| {
            Ok((
                QueueItem {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    agent_id: r.get(2)?,
                    agent_role: r.get(3)?,
                    status: r.get(4)?,
                    claimed_at: r.get(5)?,
                    started_at: r.get(6)?,
                    completed_at: r.get(7)?,
                    result: r.get(8)?,
                    error: r.get(9)?,
                    retry_count: r.get(10)?,
                    max_retries: r.get(11)?,
                    created_at: r.get(12)?,
                    dag_id: None,
                    node_id: None,
                },
                r.get::<_, String>(13)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .filter(|(q, ws)| workspace_id.map_or(true, |w| w == ws) && status.map_or(true, |s| q.status == s))
        .map(|(q, _)| q)
        .collect();
    Ok(rows)
}

// ─── The one-time fold ──────────────────────────────────────────────────────

/// What [`fold_legacy`] did (or would do with `dry_run`).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FoldReport {
    pub dry_run: bool,
    pub tasks_seen: usize,
    /// Open tasks that became nodes (or would).
    pub tasks_folded: usize,
    /// Tasks that already were nodes (a re-run folds nothing).
    pub tasks_already_nodes: usize,
    /// Done/closed tasks left as readable history in the `tasks` table.
    pub tasks_kept_as_history: usize,
    pub queue_seen: usize,
    /// Open queue rows that became claims on their node (or would).
    pub queue_folded: usize,
    pub queue_already_nodes: usize,
    /// Done/failed queue rows left as readable history.
    pub queue_kept_as_history: usize,
    /// Rows that couldn't be folded, with the reason.
    pub skipped: Vec<FoldSkip>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FoldSkip {
    pub table: &'static str,
    pub id: String,
    pub reason: String,
}

struct LegacyQueue {
    id: String,
    task_id: String,
    agent_id: Option<String>,
    agent_role: Option<String>,
    status: String,
    claimed_at: Option<String>,
    started_at: Option<String>,
    retry_count: i64,
    max_retries: i64,
    created_at: String,
}

fn read_legacy_tasks(conn: &rusqlite::Connection) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, workspace_id, title, description, status, priority, assignee_id, due_date, tags, metadata,
                created_at, updated_at, assignee_type, assignee_name FROM tasks ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], crate::task_routes::row_to_task)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn read_legacy_queue(conn: &rusqlite::Connection) -> Result<Vec<LegacyQueue>> {
    let mut stmt = match conn.prepare(
        "SELECT id, task_id, agent_id, agent_role, status, claimed_at, started_at, COALESCE(retry_count, 0), COALESCE(max_retries, 3), COALESCE(created_at, '')
         FROM cowork_queue ORDER BY created_at",
    ) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("no such table") => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let rows = stmt
        .query_map([], |r| {
            Ok(LegacyQueue {
                id: r.get(0)?,
                task_id: r.get(1)?,
                agent_id: r.get(2)?,
                agent_role: r.get(3)?,
                status: r.get(4)?,
                claimed_at: r.get(5)?,
                started_at: r.get(6)?,
                retry_count: r.get(7)?,
                max_retries: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Fold every open cowork task and queue row into nodes, through the Gate.
/// Idempotent: a task that already is a node is counted, not rewritten.
/// With `dry_run`, nothing is written and the report says what would be.
pub async fn fold_legacy(conn: &rusqlite::Connection, rails: &RailsState, dry_run: bool) -> Result<FoldReport> {
    let _guard = WRITE_LOCK.lock().await;
    let mut report = FoldReport { dry_run, ..Default::default() };
    let tasks = read_legacy_tasks(conn)?;
    let queue = read_legacy_queue(conn)?;
    let events = ledger_events(rails).await.map_err(|e| anyhow!(e.fact))?;
    let by_id: HashMap<String, &Task> = tasks.iter().map(|t| (t.id.clone(), t)).collect();
    // Tasks with an open queue row are folded even when the task row itself
    // says done, so no claim is lost.
    let queued_open: std::collections::HashSet<&str> =
        queue.iter().filter(|q| queue_open(&q.status)).map(|q| q.task_id.as_str()).collect();

    report.tasks_seen = tasks.len();
    for t in &tasks {
        if find(&events, &t.user_id, &t.id).is_some() {
            report.tasks_already_nodes += 1;
            continue;
        }
        let closed = TaskStatus::parse(&t.status).map_or(false, TaskStatus::is_closed);
        if closed && !queued_open.contains(t.id.as_str()) {
            report.tasks_kept_as_history += 1;
            continue;
        }
        if !valid_task_id(&t.id) {
            report.skipped.push(FoldSkip { table: "tasks", id: t.id.clone(), reason: "the id can't be a node id".into() });
            continue;
        }
        if t.title.trim().is_empty() {
            report.skipped.push(FoldSkip { table: "tasks", id: t.id.clone(), reason: "the task has no title".into() });
            continue;
        }
        if !dry_run {
            if let Err(e) = fold_row_locked(rails, t).await {
                report.skipped.push(FoldSkip { table: "tasks", id: t.id.clone(), reason: e.to_string() });
                continue;
            }
        }
        report.tasks_folded += 1;
    }

    report.queue_seen = queue.len();
    let events = if dry_run { events } else { ledger_events(rails).await.map_err(|e| anyhow!(e.fact))? };
    for q in &queue {
        if !queue_open(&q.status) {
            report.queue_kept_as_history += 1;
            continue;
        }
        let Some(task) = by_id.get(&q.task_id) else {
            report.skipped.push(FoldSkip { table: "cowork_queue", id: q.id.clone(), reason: format!("its task {} doesn't exist", q.task_id) });
            continue;
        };
        if dry_run {
            let skipped_task = report.skipped.iter().any(|s| s.table == "tasks" && s.id == task.id);
            if skipped_task {
                report.skipped.push(FoldSkip { table: "cowork_queue", id: q.id.clone(), reason: format!("its task {} can't be folded", task.id) });
            } else if find(&events, &task.user_id, &task.id).and_then(|(d, n)| queue_of(&d, &n)).is_some() {
                report.queue_already_nodes += 1;
            } else {
                report.queue_folded += 1;
            }
            continue;
        }
        let Some((dag, node)) = find(&events, &task.user_id, &task.id) else {
            report.skipped.push(FoldSkip { table: "cowork_queue", id: q.id.clone(), reason: format!("its task {} isn't a node", task.id) });
            continue;
        };
        if queue_of(&dag, &node).is_some() {
            report.queue_already_nodes += 1;
            continue;
        }
        let nid = node.node_id.clone();
        let target = if q.status == "pending" { "READY" } else { "IN_PROGRESS" };
        let muts = (|| -> CoworkResult<Vec<DagMutation>> {
            // A closed task with an open claim is reopened for that claim.
            let mut m = status_changes(&nid, &node.status, if node_closed(&node.status) { "NEW" } else { &node.status })?;
            let from = if node_closed(&node.status) { "NEW" } else { node.status.as_str() };
            m.extend(status_changes(&nid, from, target)?);
            if q.status != "pending" {
                if let Some(a) = q.agent_id.as_deref().filter(|a| !a.is_empty()) {
                    m.push(DagMutation::UpdateNode { node_id: nid.clone(), patch: json!({ "assignee": a }) });
                }
            }
            for (k, v) in [
                ("queue_id", q.id.as_str()),
                ("queue_status", q.status.as_str()),
                ("queue_agent_id", q.agent_id.as_deref().unwrap_or("")),
                ("queue_agent_role", q.agent_role.as_deref().unwrap_or("")),
                ("queue_claimed_at", q.claimed_at.as_deref().unwrap_or("")),
                ("queue_started_at", q.started_at.as_deref().unwrap_or("")),
                ("queue_retry_count", &q.retry_count.to_string()),
                ("queue_max_retries", &q.max_retries.to_string()),
                ("queue_created_at", q.created_at.as_str()),
            ] {
                m.push(set_state(&nid, k, v));
            }
            Ok(m)
        })();
        match muts {
            Ok(m) => match apply(rails, &dag, "cowork queue row folded into a node claim", m).await {
                Ok(()) => report.queue_folded += 1,
                Err(e) => report.skipped.push(FoldSkip { table: "cowork_queue", id: q.id.clone(), reason: e.to_string() }),
            },
            Err(e) => report.skipped.push(FoldSkip { table: "cowork_queue", id: q.id.clone(), reason: e.to_string() }),
        }
    }

    if !dry_run {
        // Refresh the read model for everything that's now a node.
        let events = ledger_events(rails).await.map_err(|e| anyhow!(e.fact))?;
        for t in &tasks {
            if let Some((d, n)) = find(&events, &t.user_id, &t.id) {
                let _ = write_row(conn, &task_of(&d, &n));
            }
        }
    }
    Ok(report)
}

/// Run the fold once per database: skipped when a real run is recorded in
/// `factory_cowork_fold_runs`. Rows a run couldn't fold stay readable and
/// fold the first time they're edited.
pub async fn fold_once(state: &Arc<crate::AppState>) -> Result<Option<FoldReport>> {
    let conn = state.db.connect()?;
    let done: i64 = conn
        .query_row("SELECT COUNT(*) FROM factory_cowork_fold_runs WHERE dry_run = 0", [], |r| r.get(0))
        .unwrap_or(0);
    if done > 0 {
        return Ok(None);
    }
    let report = fold_legacy(&conn, &state.rails, false).await?;
    conn.execute(
        "INSERT INTO factory_cowork_fold_runs (id, dry_run, skipped, report) VALUES (?1, 0, ?2, ?3)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), report.skipped.len() as i64, serde_json::to_string(&report)?],
    )?;
    Ok(Some(report))
}

// ─── The Tasks board (`/api/factory/tasks/*`) ───────────────────────────────

/// The user's Tasks board for one workspace (empty = personal): the engine's
/// own board over the Tasks DAG, so the app draws it like any campaign.
pub fn tasks_board(rails: &RailsState, events: &[AllternitEvent], user_id: &str, workspace_id: &str) -> CoworkResult<Value> {
    let dag = dag_id(user_id, workspace_id);
    let title = if workspace_id.trim().is_empty() { "Tasks".to_string() } else { format!("Tasks ({})", workspace_id.trim()) };
    if !user_dags(events, user_id).contains(&dag) {
        // No task has been written yet: an empty board, not an error.
        return Ok(json!({
            "campaign": { "id": dag, "title": title, "intent": title },
            "summary": { "now": [], "next": [], "proven": { "k": 0, "n": 0 }, "needsYou": [] },
            "waves": [],
        }));
    }
    let mut b = board::build(&rails.root_dir, events, &dag).map_err(|e| CoworkError::not_found(e.to_string()))?;
    b.campaign.title = title.clone();
    b.campaign.intent = title;
    serde_json::to_value(b).map_err(|e| CoworkError::new("internal", e.to_string(), "Retry."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_scoped() {
        assert_eq!(dag_id("u1", ""), dag_id("u1", "  "));
        assert!(dag_id("u1", "").ends_with("-personal"));
        assert_ne!(dag_id("u1", "ws"), dag_id("u2", "ws"));
        assert!(dag_id("u1", "ws").starts_with(&user_prefix("u1")));
        assert!(valid_task_id("3f2a-01"));
        assert!(!valid_task_id("../x"));
        assert!(!valid_task_id(""));
    }

    #[test]
    fn statuses_parse_and_map() {
        assert_eq!(TaskStatus::parse("in_progress"), Some(TaskStatus::InProgress));
        assert_eq!(TaskStatus::parse("Completed"), Some(TaskStatus::Done));
        assert_eq!(TaskStatus::parse("whatever"), None);
        assert_eq!(target_node_status(TaskStatus::Backlog, "NEW"), "NEW");
        assert_eq!(target_node_status(TaskStatus::Backlog, "IN_PROGRESS"), "READY");
        assert_eq!(target_node_status(TaskStatus::InReview, "READY"), "VERIFYING");
    }

    #[test]
    fn status_changes_reopen_closed_nodes() {
        assert!(status_changes("n", "READY", "READY").unwrap().is_empty());
        assert_eq!(status_changes("n", "READY", "IN_PROGRESS").unwrap().len(), 1);
        let reopen = status_changes("n", "DONE", "IN_PROGRESS").unwrap();
        assert_eq!(reopen.len(), 2);
        assert!(matches!(&reopen[0], DagMutation::ChangeStatus { to, .. } if to == "NEW"));
    }

    // ------------------------------------------------------------ ledger + routes

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt;

    async fn setup() -> Arc<crate::AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-cowork-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        allternit_factory_engine::policy::inject_policy(&state.rails.root_dir, &state.rails.ledger, None, "gateway").await.unwrap();
        state
    }

    /// Cowork rows as they were before the fold: open, closed, queued.
    fn seed(state: &crate::AppState) {
        let c = state.db.connect().unwrap();
        for (id, user, ws, title, status) in [
            ("t-open", "user-a", "ws1", "Write the brief", "todo"),
            ("t-doing", "user-a", "ws1", "Draft the deck", "in-progress"),
            ("t-done", "user-a", "ws1", "Old work", "done"),
            ("t-done-queued", "user-a", "", "Closed but claimed", "done"),
            ("t-bad id", "user-a", "", "Bad id", "todo"),
            ("t-other", "user-b", "", "Someone else's", "backlog"),
        ] {
            c.execute(
                "INSERT INTO tasks (id, user_id, workspace_id, title, status, priority) VALUES (?1, ?2, ?3, ?4, ?5, '50')",
                rusqlite::params![id, user, ws, title, status],
            )
            .unwrap();
        }
        for (id, task, status) in [("q-pending", "t-open", "pending"), ("q-claimed", "t-done-queued", "claimed"), ("q-old", "t-done", "completed")] {
            c.execute("INSERT INTO cowork_queue (id, task_id, agent_id, status) VALUES (?1, ?2, 'bot-a', ?3)", rusqlite::params![id, task, status]).unwrap();
        }
    }

    fn app(state: &Arc<crate::AppState>) -> Router {
        Router::new()
            .nest("/api/v1", crate::task_routes::task_router())
            .nest("/api/v1", crate::queue_routes::queue_router())
            .nest("/api", crate::factory_tasks::router())
            .with_state(state.clone())
    }

    async fn call(app: &Router, method: &str, uid: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("x-allternit-user-id", uid)
            .body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    static SERIAL: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

    #[tokio::test]
    async fn fold_dry_run_then_real_run_then_rerun_is_idempotent() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        seed(&state);
        let conn = state.db.connect().unwrap();

        let dry = fold_legacy(&conn, &state.rails, true).await.unwrap();
        assert!(dry.dry_run);
        assert_eq!(dry.tasks_seen, 6);
        // t-open, t-doing, t-done-queued (open claim), t-other; t-done is history; "t-bad id" is skipped.
        assert_eq!(dry.tasks_folded, 4, "{dry:?}");
        assert_eq!(dry.tasks_kept_as_history, 1);
        assert_eq!(dry.queue_folded, 2);
        assert_eq!(dry.queue_kept_as_history, 1);
        assert_eq!(dry.skipped.len(), 1);
        assert!(user_task_nodes(&ledger_events(&state.rails).await.unwrap(), "user-a", None).is_empty(), "dry run wrote nothing");

        let real = fold_legacy(&conn, &state.rails, false).await.unwrap();
        assert_eq!((real.tasks_folded, real.queue_folded, real.skipped.len()), (4, 2, 1), "{real:?}");
        let events = ledger_events(&state.rails).await.unwrap();
        let (_, doing) = find(&events, "user-a", "t-doing").unwrap();
        assert_eq!(doing.status, "IN_PROGRESS");
        let (_, reopened) = find(&events, "user-a", "t-done-queued").unwrap();
        assert_eq!(reopened.status, "IN_PROGRESS", "a closed task with an open claim is reopened for it");
        assert_eq!(reopened.assignee.as_deref(), Some("bot-a"));
        assert!(find(&events, "user-a", "t-done").is_none(), "closed history stays a row");
        assert!(find(&events, "user-a", "t-other").is_none(), "nodes are scoped to their owner");
        assert!(find(&events, "user-b", "t-other").is_some());

        let again = fold_legacy(&conn, &state.rails, false).await.unwrap();
        assert_eq!((again.tasks_folded, again.queue_folded), (0, 0), "{again:?}");
        assert_eq!((again.tasks_already_nodes, again.queue_already_nodes), (4, 2));
    }

    #[tokio::test]
    async fn routes_keep_their_shapes_and_write_nodes() {
        let _g = SERIAL.lock().await;
        let state = setup().await;
        seed(&state);
        let app = app(&state);

        // Create, with a dry run first.
        let (st, plan) = call(&app, "POST", "user-a", "/api/v1/tasks?dryRun=true", Some(json!({ "id": "t-new", "title": "Ship it", "workspace_id": "ws1" }))).await;
        assert_eq!(st, StatusCode::OK, "{plan}");
        assert_eq!(plan["dryRun"], true);
        assert!(find(&ledger_events(&state.rails).await.unwrap(), "user-a", "t-new").is_none());
        let (st, created) = call(&app, "POST", "user-a", "/api/v1/tasks", Some(json!({ "id": "t-new", "title": "Ship it", "workspace_id": "ws1" }))).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["task"]["status"], "todo");
        assert_eq!(created["task"]["nodeId"], "task-t-new");
        let (st, _) = call(&app, "POST", "user-a", "/api/v1/tasks", Some(json!({ "id": "t-new", "title": "Ship it" }))).await;
        assert_eq!(st, StatusCode::OK, "a repeated id returns the same task");

        // List: nodes plus pre-fold history (t-done), only the caller's.
        let (_, list) = call(&app, "GET", "user-a", "/api/v1/tasks?workspace_id=ws1", None).await;
        let ids: Vec<&str> = list["tasks"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"t-new") && ids.contains(&"t-done") && ids.contains(&"t-open"), "{ids:?}");
        let (st, _) = call(&app, "GET", "user-b", "/api/v1/tasks/t-new", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Editing a pre-fold row folds it.
        let (st, t) = call(&app, "PUT", "user-a", "/api/v1/tasks/t-open", Some(json!({ "status": "backlog" }))).await;
        assert_eq!(st, StatusCode::OK, "{t}");
        assert_eq!(t["status"], "backlog");
        assert!(t["nodeId"].is_string());

        // Queue: enqueue → claim → start → complete puts the task in review, not done.
        let (st, q) = call(&app, "POST", "user-a", "/api/v1/queue", Some(json!({ "id": "q-new", "task_id": "t-new" }))).await;
        assert_eq!(st, StatusCode::CREATED, "{q}");
        let (st, _) = call(&app, "POST", "user-a", "/api/v1/queue", Some(json!({ "id": "q-dup", "task_id": "t-new" }))).await;
        assert_eq!(st, StatusCode::CONFLICT, "one open queue item per node");
        let (_, claimed) = call(&app, "POST", "user-a", "/api/v1/queue/claim", Some(json!({ "agent_id": "bot-a", "workspace_id": "ws1" }))).await;
        assert_eq!(claimed["id"], "q-new");
        assert_eq!(claimed["status"], "claimed");
        let (_, nothing) = call(&app, "POST", "user-b", "/api/v1/queue/claim", Some(json!({ "agent_id": "bot-b" }))).await;
        assert!(nothing.is_null(), "user-b can't claim user-a's work");
        let (_, _) = call(&app, "POST", "user-a", "/api/v1/queue/q-new/start", None).await;
        let (st, done) = call(&app, "POST", "user-a", "/api/v1/queue/q-new/complete", Some(json!({ "result": "ok" }))).await;
        assert_eq!(st, StatusCode::OK, "{done}");
        assert_eq!(done["status"], "completed");
        let (_, t) = call(&app, "GET", "user-a", "/api/v1/tasks/t-new", None).await;
        assert_eq!(t["status"], "in-review", "the worker's word never closes a task");

        // The read model follows the node.
        let row = legacy_row(&state.db.connect().unwrap(), "t-new").unwrap().unwrap();
        assert_eq!(row.status, "in-review");

        // The Tasks board and node page.
        let (st, board) = call(&app, "GET", "user-a", "/api/factory/tasks/board?workspace=ws1", None).await;
        assert_eq!(st, StatusCode::OK, "{board}");
        let cards: Vec<&str> = board["waves"].as_array().unwrap().iter().flat_map(|w| w["nodes"].as_array().unwrap()).map(|c| c["nodeId"].as_str().unwrap()).collect();
        assert!(cards.contains(&"task-t-new"), "{board}");
        let (st, empty) = call(&app, "GET", "user-b", "/api/factory/tasks/board?workspace=nope", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(empty["waves"], json!([]));
        let (st, page) = call(&app, "GET", "user-a", "/api/factory/tasks/nodes/task-t-new", None).await;
        assert_eq!(st, StatusCode::OK, "{page}");
        let (st, _) = call(&app, "GET", "user-b", "/api/factory/tasks/nodes/task-t-new", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Close, then delete.
        let (_, t) = call(&app, "PUT", "user-a", "/api/v1/tasks/t-new", Some(json!({ "status": "done" }))).await;
        assert_eq!(t["status"], "done");
        let (st, _) = call(&app, "DELETE", "user-a", "/api/v1/tasks/t-new", None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        assert!(find(&ledger_events(&state.rails).await.unwrap(), "user-a", "t-new").is_none());
    }
}
