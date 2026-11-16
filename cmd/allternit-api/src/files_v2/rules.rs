//! Folder rules: the owner's pre-approval for a folder (SPEC section 7).
//!
//! A rule says "when a file lands in this root, run the Inbox task with this
//! fixed tool set, no approval each time". Rules live in Automation Tasks,
//! the same `routines` table the Automation Tasks screen lists, so the owner
//! sees, pauses and deletes them there:
//!
//! - `execution_domain = 'files'`: the local routine scheduler, the startup
//!   pass, the "needs a bot" sweep and the gizzi cron daemon all select
//!   `local` / `cloud` rows only, so they never run a rule on a schedule.
//! - `schedule_type = 'manual'`, `config.trigger = "files.drop"` (the trigger
//!   is a drop, not a time), `config.filesRule = {rootId, tools, output,
//!   approvedBy, computerId}`, `description` = the owner's instruction.
//! - Every rule run is a `routine_runs` row (`triggered_by = 'files.drop'`),
//!   so the rule's history shows in Automation Tasks.
//!
//! A rule can be added only on this computer (like loosening agent access in
//! `roots::check_policy_change`), only on a root with `agent_process` on, and
//! only with tools from `inbox::KNOWN_TOOLS`. Pausing and deleting work from
//! any of the owner's devices.
//!
//! **Sealed.** The Automation Tasks routes can replace a routine's `config`
//! and `description` from any device. So when a rule is saved here, its
//! `filesRule` and instruction are also kept in this drive's own database
//! (`folder_rule_seals`, not reachable through those routes). A rule whose
//! row no longer matches its seal is ignored, and its drops wait for
//! approval again: an edit made elsewhere can't widen the tools, move the
//! rule to another folder or change what the model is told.

use std::sync::Arc;

use allternit_files_core::{ErrorCode, PrincipalRef};
use axum::extract::rejection::JsonRejection;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Extension, Json};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::inbox::{self, blocking, owner_only, Output, Task, TaskState};
use super::{events, ApiError, Fail, FilesV2, Inner, Req};

/// `routines.execution_domain` of a folder rule.
pub const RULE_DOMAIN: &str = "files";
/// `routines.config.trigger` of a folder rule.
pub const RULE_TRIGGER: &str = "files.drop";
/// The agent identity rule tasks run as.
pub const RULE_AGENT: &str = "agt_files_rule";

/// True for a routine's `config` that is a folder rule. The local scheduler
/// skips these on a manual run: a rule runs only on a drop, with its grant.
pub fn is_folder_rule(config: &Value) -> bool {
    config.get("trigger").and_then(Value::as_str) == Some(RULE_TRIGGER)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderRule {
    pub rule_id: String,
    pub root_id: String,
    pub name: String,
    /// The owner's own words for the agent (trusted; shown outside the
    /// untrusted block).
    #[serde(default)]
    pub instruction: String,
    pub tools: Vec<String>,
    pub output: Output,
    pub active: bool,
    pub approved_by: PrincipalRef,
    pub created_at: String,
}

fn store(inner: &Inner) -> Result<&crate::db::DbHandle, ApiError> {
    inner
        .cfg
        .memory_db
        .as_ref()
        .ok_or_else(|| ApiError::unsupported("Folder rules need the Automation Tasks store, which this runtime doesn't have."))
}

const COLS: &str = "id, name, COALESCE(description, ''), status, config, created_at";

/// What a rule's seal covers: everything that decides what it may do.
fn seal(files_rule: &Value, instruction: &str) -> Value {
    json!({ "filesRule": files_rule, "instruction": instruction })
}

/// A rule row and the seal its current contents would have.
fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<(FolderRule, Value)>> {
    let config: String = r.get(4)?;
    let config: Value = serde_json::from_str(&config).unwrap_or(Value::Null);
    let rule = &config["filesRule"];
    let (Some(root_id), Some(tools), Ok(approved_by)) = (
        rule["rootId"].as_str(),
        rule["tools"].as_array(),
        serde_json::from_value::<PrincipalRef>(rule["approvedBy"].clone()),
    ) else {
        return Ok(None);
    };
    let status: String = r.get(3)?;
    let instruction: String = r.get(2)?;
    let sealed = seal(rule, &instruction);
    Ok(Some((
        FolderRule {
            rule_id: r.get(0)?,
            root_id: root_id.to_string(),
            name: r.get(1)?,
            instruction,
            tools: tools.iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
            output: Output::parse(rule["output"].as_str().unwrap_or("beside")),
            active: status == "active",
            approved_by,
            created_at: r.get(5)?,
        },
        sealed,
    )))
}

/// The seal saved when the rule was made here, if any.
fn saved_seal(inner: &Inner, rule_id: &str) -> rusqlite::Result<Option<Value>> {
    let c = inner.meta.lock().expect("meta lock");
    let s: Option<String> = c
        .query_row("SELECT seal FROM folder_rule_seals WHERE rule_id = ?1", params![rule_id], |r| r.get(0))
        .optional()?;
    Ok(s.and_then(|s| serde_json::from_str(&s).ok()))
}

fn query(inner: &Inner, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<FolderRule>, Fail> {
    let c = store(inner)?.connect()?;
    let mut s = c.prepare(&format!(
        "SELECT {COLS} FROM routines WHERE execution_domain = '{RULE_DOMAIN}' \
         AND json_extract(CASE WHEN json_valid(config) THEN config ELSE '{{}}' END, '$.trigger') = '{RULE_TRIGGER}' \
         {filter} ORDER BY created_at, id"
    ))?;
    let rows: Vec<Option<(FolderRule, Value)>> = s.query_map(args, from_row)?.collect::<rusqlite::Result<_>>()?;
    drop(s);
    drop(c);
    let mut out = Vec::new();
    for (rule, sealed) in rows.into_iter().flatten() {
        if saved_seal(inner, &rule.rule_id)?.as_ref() != Some(&sealed) {
            tracing::warn!(rule = %rule.rule_id, "files v2: folder rule changed outside the drive; ignored until it is saved again here");
            continue;
        }
        out.push(rule);
    }
    Ok(out)
}

/// The active rule for a root (the oldest when there are several).
pub(crate) fn for_root(inner: &Inner, root_id: &str) -> Option<FolderRule> {
    if inner.cfg.memory_db.is_none() {
        return None;
    }
    match query(inner, "AND status = 'active' AND json_extract(CASE WHEN json_valid(config) THEN config ELSE '{}' END, '$.filesRule.rootId') = ?1", &[&root_id]) {
        Ok(rules) => rules.into_iter().next(),
        Err(e) => {
            tracing::warn!(error = %e.into_api(inner).body.message, "files v2: folder rules not read");
            None
        }
    }
}

pub(crate) fn get(inner: &Inner, rule_id: &str) -> Option<FolderRule> {
    if inner.cfg.memory_db.is_none() {
        return None;
    }
    query(inner, "AND id = ?1", &[&rule_id]).ok().and_then(|r| r.into_iter().next())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRule {
    pub root_id: String,
    pub name: String,
    #[serde(default)]
    pub instruction: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub output: Option<Output>,
}

pub(crate) fn create(inner: &Inner, req: &Req, body: CreateRule) -> Result<FolderRule, Fail> {
    if req.relayed {
        return Err(ApiError::forbidden("Folder rules let an agent run without asking each time, so they can only be added on this computer.").into());
    }
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::invalid("Give the rule a name of 1 to 120 characters.").into());
    }
    if body.instruction.chars().count() > 4000 {
        return Err(ApiError::invalid("The instruction can be at most 4000 characters.").into());
    }
    let tools = inbox::validate_tools(body.tools)?;
    let output = body.output.unwrap_or_default();
    let root = inner.drive.root(&body.root_id)?;
    if !inbox::agent_may_use(&root) {
        return Err(ApiError::new(
            ErrorCode::AgentProcessDenied,
            "Turn on agent access for this folder first. Sharing a folder doesn't turn it on.",
        )
        .into());
    }
    let rule = FolderRule {
        rule_id: uuid::Uuid::new_v4().to_string(),
        root_id: root.root_id.clone(),
        name,
        instruction: body.instruction.trim().to_string(),
        tools,
        output,
        active: true,
        approved_by: req.principal.clone(),
        created_at: events::now(inner),
    };
    let config = json!({
        "trigger": RULE_TRIGGER,
        "filesRule": {
            "rootId": rule.root_id,
            "tools": rule.tools,
            "output": rule.output.as_str(),
            "approvedBy": rule.approved_by,
            "computerId": inner.cfg.computer_id,
        }
    });
    let db = store(inner)?;
    inner.meta.lock().expect("meta lock").execute(
        "INSERT INTO folder_rule_seals(rule_id, seal) VALUES(?1, ?2)",
        params![rule.rule_id, seal(&config["filesRule"], &rule.instruction).to_string()],
    )?;
    let written = db.connect().and_then(|c| {
        c.execute(
            "INSERT INTO routines (id, user_id, agent_id, name, description, status, schedule_type, schedule_expression, \
             execution_domain, config, metadata, created_at, updated_at) \
             VALUES (?1, ?2, NULL, ?3, ?4, 'active', 'manual', '', ?5, ?6, '{}', ?7, ?7)",
            params![rule.rule_id, req.user_id, rule.name, rule.instruction, RULE_DOMAIN, config.to_string(), rule.created_at],
        )
    });
    if let Err(e) = written {
        forget_seal(inner, &rule.rule_id);
        return Err(e.into());
    }
    Ok(rule)
}

pub(crate) fn list(inner: &Inner) -> Result<Vec<FolderRule>, Fail> {
    query(inner, "", &[])
}

fn rule_not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "That folder rule doesn't exist.")
}

pub(crate) fn set_active(inner: &Inner, rule_id: &str, active: bool) -> Result<FolderRule, Fail> {
    let n = store(inner)?.connect()?.execute(
        "UPDATE routines SET status = ?2, updated_at = ?3 WHERE id = ?1 AND execution_domain = ?4",
        params![rule_id, if active { "active" } else { "paused" }, events::now(inner), RULE_DOMAIN],
    )?;
    if n == 0 {
        return Err(rule_not_found().into());
    }
    Ok(get(inner, rule_id).ok_or_else(rule_not_found)?)
}

pub(crate) fn delete(inner: &Inner, rule_id: &str) -> Result<(), Fail> {
    let n = store(inner)?
        .connect()?
        .execute("DELETE FROM routines WHERE id = ?1 AND execution_domain = ?2", params![rule_id, RULE_DOMAIN])?;
    if n == 0 {
        return Err(rule_not_found().into());
    }
    forget_seal(inner, rule_id);
    Ok(())
}

fn forget_seal(inner: &Inner, rule_id: &str) {
    if let Err(e) = inner.meta.lock().expect("meta lock").execute("DELETE FROM folder_rule_seals WHERE rule_id = ?1", params![rule_id]) {
        tracing::warn!(error = %e, rule = %rule_id, "files v2: folder rule seal not removed");
    }
}

/// One `routine_runs` row per finished rule task (Automation Tasks history).
pub(crate) fn record_run(inner: &Inner, rule_id: &str, task: &Task) {
    let Some(db) = inner.cfg.memory_db.as_ref() else { return };
    let status = if task.state == TaskState::Succeeded { "succeeded" } else { "failed" };
    let output = (task.state == TaskState::Succeeded).then(|| format!("Summarized {}", task.name));
    let metadata = json!({ "taskId": task.task_id, "itemId": task.item_id, "resultItemIds": task.result_item_ids });
    let written = db.connect().and_then(|c| {
        c.execute(
            "INSERT INTO routine_runs (id, routine_id, status, scheduled_at, started_at, finished_at, output, error, triggered_by, metadata) \
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                uuid::Uuid::new_v4().to_string(),
                rule_id,
                status,
                task.created_at,
                task.updated_at,
                output,
                task.error,
                RULE_TRIGGER,
                metadata.to_string()
            ],
        )?;
        c.execute("UPDATE routines SET last_run_at = ?2 WHERE id = ?1", params![rule_id, task.updated_at])
    });
    if let Err(e) = written {
        tracing::warn!(error = %e, rule = %rule_id, "files v2: folder rule run not recorded");
    }
}

// ---- routes (registered by `inbox::routes`) ----

type Files = Extension<Arc<FilesV2>>;

#[derive(Serialize)]
pub(crate) struct RulesResponse {
    rules: Vec<FolderRule>,
}

pub(crate) async fn list_route(Extension(files): Files, req: Req) -> Result<Json<RulesResponse>, ApiError> {
    owner_only(&req)?;
    let inner = files.inner().await?;
    let rules = blocking(&inner, list).await?;
    Ok(Json(RulesResponse { rules }))
}

pub(crate) async fn create_route(
    Extension(files): Files,
    req: Req,
    body: Result<Json<CreateRule>, JsonRejection>,
) -> Result<(StatusCode, Json<FolderRule>), ApiError> {
    owner_only(&req)?;
    let Json(body) = body?;
    let inner = files.inner().await?;
    let rule = blocking(&inner, move |v| create(v, &req, body)).await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateRule {
    active: bool,
}

pub(crate) async fn update_route(
    Extension(files): Files,
    req: Req,
    Path(rule_id): Path<String>,
    body: Result<Json<UpdateRule>, JsonRejection>,
) -> Result<Json<FolderRule>, ApiError> {
    owner_only(&req)?;
    let Json(body) = body?;
    if body.active && req.relayed {
        return Err(ApiError::forbidden("A paused folder rule can only be turned back on from this computer."));
    }
    let inner = files.inner().await?;
    let rule = blocking(&inner, move |v| set_active(v, &rule_id, body.active)).await?;
    Ok(Json(rule))
}

pub(crate) async fn delete_route(Extension(files): Files, req: Req, Path(rule_id): Path<String>) -> Result<StatusCode, ApiError> {
    owner_only(&req)?;
    let inner = files.inner().await?;
    blocking(&inner, move |v| delete(v, &rule_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

