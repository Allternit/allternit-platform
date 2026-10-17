//! Restore a ledger (a downloaded `allternit-ledger.zip`, or the folder or
//! git copy) into this account on this computer: ledger program phase 4,
//! step 2. Preview first, then apply.
//!
//! - Records keep their ids. One that's already yours is "already here"; one
//!   another account on this computer uses is skipped with the reason.
//! - Only columns the table has here are written, so ledgers from older or
//!   newer versions still restore.
//! - Routines, scheduled messages, schedules, loops and deployments come back
//!   paused, so nothing starts running on a new computer by itself.
//! - Values the ledger left out (credentials) are dropped, never written back,
//!   and listed so the person can fill them in again.
//! - Bot workspace files and skill folders are only ever added: an existing
//!   file is never overwritten.
//! - Connected apps can't be restored (sign-ins are never in a ledger); they
//!   are listed to reconnect.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::db::DbHandle;
use crate::memory_drive_service::{Result, ServiceError};

const LEFT_OUT_PREFIX: &str = "[left out";
pub const MAX_FILES: usize = 5000;

/// Where one ledger folder goes, and how it comes back.
struct Target {
    /// Ledger path prefix, e.g. `routines/` (records directly inside it).
    dir: &'static str,
    table: &'static str,
    /// Column overrides so it comes back paused.
    pause: &'static [(&'static str, &'static str)],
    label: &'static str,
}

const TARGETS: &[Target] = &[
    Target { dir: "bot-templates/", table: "bot_templates", pause: &[], label: "bot template" },
    Target { dir: "routines/scheduled-messages/", table: "cowork_routines", pause: &[("enabled", "0"), ("next_run_at", "NULL")], label: "scheduled message" },
    Target { dir: "routines/", table: "routines", pause: &[("status", "'paused'"), ("next_run_at", "NULL")], label: "routine" },
    Target { dir: "workflows/", table: "workflows", pause: &[], label: "workflow" },
    Target { dir: "tasks/", table: "tasks", pause: &[], label: "task" },
    Target { dir: "skills/recipes/", table: "skills", pause: &[], label: "skill recipe" },
    Target { dir: "projects/", table: "cowork_projects", pause: &[], label: "project" },
    Target { dir: "schedules/loops/", table: "loops", pause: &[("status", "'paused'")], label: "loop" },
    Target { dir: "schedules/deployments/", table: "beta_deployments", pause: &[("status", "'paused'"), ("next_run_at", "NULL")], label: "scheduled run" },
    Target { dir: "schedules/", table: "cowork_scheduled_tasks", pause: &[("enabled", "0"), ("next_run_at", "NULL")], label: "scheduled task" },
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Item {
    pub kind: &'static str,
    pub path: String,
    pub name: String,
    /// `add`, `already_here` or `skip`.
    pub action: &'static str,
    pub reason: Option<String>,
    /// Settings the ledger left out that need filling in again.
    pub needs: Vec<String>,
    pub paused: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Plan {
    pub items: Vec<Item>,
    /// Files added to bot workspaces and skill folders.
    pub files_added: usize,
    pub files_kept: usize,
    /// Connected apps to reconnect: `{connector, account}`.
    pub reconnect: Vec<Value>,
}

/// One record found in the ledger, ready to insert.
struct Record<'a> {
    target: &'a Target,
    path: String,
    row: Map<String, Value>,
    needs: Vec<String>,
}

/// Remove left-out values (anywhere), noting where they were.
pub(crate) fn strip_left_out(v: &mut Value, at: &str, needs: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            let gone: Vec<String> = o.iter().filter(|(_, x)| x.as_str().is_some_and(|s| s.starts_with(LEFT_OUT_PREFIX))).map(|(k, _)| k.clone()).collect();
            for k in gone {
                o.remove(&k);
                needs.push(if at.is_empty() { k } else { format!("{at}.{k}") });
            }
            for (k, x) in o.iter_mut() {
                let here = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
                strip_left_out(x, &here, needs);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| strip_left_out(x, at, needs)),
        _ => {}
    }
}

fn target_for(path: &str) -> Option<&'static Target> {
    if !path.ends_with(".json") {
        return None;
    }
    // Most specific folder first; records sit directly inside it.
    TARGETS.iter().filter(|t| path.starts_with(t.dir) && !path[t.dir.len()..].contains('/')).max_by_key(|t| t.dir.len())
}

fn name_of(row: &Map<String, Value>) -> String {
    ["name", "title"]
        .iter()
        .find_map(|k| row.get(*k).and_then(|v| v.as_str()))
        .or_else(|| row.get("template").and_then(|t| t.get("name")).and_then(|v| v.as_str()))
        .or_else(|| row.get("id").and_then(|v| v.as_str()))
        .unwrap_or("untitled")
        .to_string()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1)).map(|it| it.filter_map(|r| r.ok()).collect()))
        .unwrap_or_default()
}

fn owner_of(conn: &Connection, table: &str, id: &str) -> Option<Option<String>> {
    conn.query_row(&format!("SELECT user_id FROM {table} WHERE id = ?1"), [id], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten()
}

fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(SqlValue::Integer).or_else(|| n.as_f64().map(SqlValue::Real)).unwrap_or(SqlValue::Null),
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// Insert one record for `owner`, paused where its kind runs on a schedule.
fn insert(conn: &Connection, owner: &str, rec: &Record<'_>) -> rusqlite::Result<()> {
    let cols = columns(conn, rec.target.table);
    let mut names = vec!["user_id".to_string()];
    let mut values: Vec<String> = vec!["?".into()];
    let mut params: Vec<SqlValue> = vec![SqlValue::Text(owner.into())];
    for (k, v) in &rec.row {
        if k == "user_id" || !cols.contains(k) || rec.target.pause.iter().any(|(c, _)| c == k) {
            continue;
        }
        names.push(k.clone());
        values.push("?".into());
        params.push(to_sql(v));
    }
    for (c, literal) in rec.target.pause {
        if cols.iter().any(|x| x == c) {
            names.push((*c).into());
            values.push((*literal).into());
        }
    }
    let sql = format!("INSERT INTO {} ({}) VALUES ({})", rec.target.table, names.join(", "), values.join(", "));
    conn.execute(&sql, params_from_iter(params)).map(|_| ())
}

/// Insert a new record into `table` for `owner`, paused where that kind runs
/// on a schedule (the same rules a restore uses). Shared with ledger edits
/// that create a new thing.
pub(crate) fn insert_new(conn: &Connection, owner: &str, table: &'static str, row: &Map<String, Value>) -> rusqlite::Result<()> {
    static AGENTS: Target = Target { dir: "bots/", table: "agents", pause: &[], label: "bot" };
    let target = TARGETS.iter().find(|t| t.table == table).unwrap_or(&AGENTS);
    let target = if target.table == table { target } else { return Err(rusqlite::Error::InvalidQuery) };
    insert(conn, owner, &Record { target, path: String::new(), row: row.clone(), needs: vec![] })
}

/// A relative path inside the ledger, safe to join under a folder.
fn safe_rel(rel: &str) -> bool {
    crate::ledger_repo::valid_path(rel)
}

struct FileWrite {
    target: PathBuf,
    text: String,
}

/// Work out (and, when `apply`, do) the restore of `files` (ledger-relative
/// path → text) for `owner`.
pub fn restore(
    conn: &Connection,
    owner: &str,
    files: &BTreeMap<String, String>,
    skills_root: Option<&Path>,
    bot_workspace: impl Fn(&str) -> PathBuf,
    apply: bool,
) -> Result<Plan> {
    if files.len() > MAX_FILES {
        return Err(ServiceError::Provenance(format!("A ledger can have up to {MAX_FILES} files.")));
    }
    let mut plan = Plan::default();
    let mut records: Vec<Record<'_>> = Vec::new();
    // Bots and agents first: their workspace files need the id.
    let mut bot_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut agent_records: Vec<(String, String, Map<String, Value>, Vec<String>)> = Vec::new();
    for (path, text) in files {
        if !safe_rel(path) {
            continue;
        }
        let parts: Vec<&str> = path.split('/').collect();
        let is_agent = matches!(parts.as_slice(), ["bots", _, "bot.json"] | ["agents", _, "agent.json"]);
        if !is_agent && target_for(path).is_none() {
            continue;
        }
        let Ok(mut v) = serde_json::from_str::<Value>(text) else {
            plan.items.push(Item { kind: "file", path: path.clone(), name: path.clone(), action: "skip", reason: Some("isn't valid JSON".into()), needs: vec![], paused: false });
            continue;
        };
        let mut needs = vec![];
        strip_left_out(&mut v, "", &mut needs);
        let Value::Object(row) = v else { continue };
        if is_agent {
            if let Some(id) = row.get("id").and_then(|x| x.as_str()) {
                bot_ids.insert(format!("{}/{}", parts[0], parts[1]), id.to_string());
            }
            agent_records.push((path.clone(), parts[0].to_string(), row, needs));
        } else if let Some(t) = target_for(path) {
            records.push(Record { target: t, path: path.clone(), row, needs });
        }
    }
    static AGENTS: Target = Target { dir: "bots/", table: "agents", pause: &[], label: "bot" };
    static PLAIN_AGENTS: Target = Target { dir: "agents/", table: "agents", pause: &[], label: "agent" };
    let mut all: Vec<Record<'_>> = agent_records
        .into_iter()
        .map(|(path, dir, row, needs)| Record { target: if dir == "bots" { &AGENTS } else { &PLAIN_AGENTS }, path, row, needs })
        .collect();
    all.extend(records);

    for rec in &all {
        let name = name_of(&rec.row);
        let id = rec.row.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let paused = !rec.target.pause.is_empty();
        let mut item = Item { kind: rec.target.label, path: rec.path.clone(), name, action: "add", reason: None, needs: rec.needs.clone(), paused };
        if id.is_empty() || id.len() > 200 || id.contains('/') || id.contains("..") || id.chars().any(char::is_control) {
            item.action = "skip";
            item.reason = Some("has a missing or unusual id".into());
        } else if columns(conn, rec.target.table).is_empty() {
            item.action = "skip";
            item.reason = Some("this version of Allternit doesn't have that kind yet".into());
        } else {
            match owner_of(conn, rec.target.table, &id) {
                Some(Some(o)) if o == owner => item.action = "already_here",
                Some(_) => {
                    item.action = "skip";
                    item.reason = Some("another account on this computer already has it".into());
                }
                None if apply => {
                    if let Err(e) = insert(conn, owner, rec) {
                        item.action = "skip";
                        item.reason = Some(format!("couldn't be restored: {e}"));
                    }
                }
                None => {}
            }
        }
        plan.items.push(item);
    }

    // Files: bot workspaces and installed skills, added but never overwritten.
    let mut writes: Vec<FileWrite> = Vec::new();
    for (path, text) in files {
        if !safe_rel(path) {
            continue;
        }
        let parts: Vec<&str> = path.splitn(4, '/').collect();
        let target = match parts.as_slice() {
            [dir @ ("bots" | "agents"), stem, "workspace", rest] => {
                let Some(id) = bot_ids.get(&format!("{dir}/{stem}")) else { continue };
                if !safe_rel(id) {
                    continue;
                }
                bot_workspace(id).join(rest)
            }
            ["skills", name, ..] if *name != "recipes" && parts.len() >= 3 => {
                let Some(root) = skills_root else { continue };
                root.join(path.strip_prefix("skills/").unwrap_or(path))
            }
            _ => continue,
        };
        if target.exists() {
            plan.files_kept += 1;
        } else {
            plan.files_added += 1;
            writes.push(FileWrite { target, text: text.clone() });
        }
    }
    if apply {
        for w in writes {
            if let Some(parent) = w.target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ServiceError::Provenance(format!("Couldn't write files: {e}")))?;
            }
            std::fs::write(&w.target, w.text).map_err(|e| ServiceError::Provenance(format!("Couldn't write files: {e}")))?;
        }
    }

    if let Some(apps) = files.get("connected-apps.json").and_then(|t| serde_json::from_str::<Vec<Value>>(t).ok()) {
        plan.reconnect = apps
            .into_iter()
            .map(|a| serde_json::json!({ "connector": a.get("connector").cloned().unwrap_or(Value::Null), "account": a.get("account").cloned().unwrap_or(Value::Null) }))
            .collect();
    }
    Ok(plan)
}

/// Restore for `owner` through the API: their skills folder and bot workspaces.
pub fn restore_for(db: &DbHandle, owner: &str, files: &BTreeMap<String, String>, apply: bool) -> Result<Plan> {
    let conn = db.connect()?;
    let skills = crate::ledger::skill_dirs().into_iter().next();
    restore(&conn, owner, files, skills.as_deref(), crate::agent_workspace_paths::workspace_dir_for, apply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE agents (id TEXT PRIMARY KEY, user_id TEXT, name TEXT, config TEXT, is_bot INTEGER, archived_at TEXT);
             CREATE TABLE routines (id TEXT PRIMARY KEY, user_id TEXT, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'active', schedule_expression TEXT, next_run_at TEXT);
             CREATE TABLE workflows (id TEXT PRIMARY KEY, user_id TEXT, title TEXT, nodes TEXT, edges TEXT);
             INSERT INTO workflows VALUES ('wf_mine','u1','Mine','[]','[]');
             INSERT INTO workflows VALUES ('wf_theirs','u2','Theirs','[]','[]');",
        )
        .unwrap();
        c
    }

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect()
    }

    #[test]
    fn previews_then_restores_paused_without_left_out_values() {
        let conn = db();
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let skills = tmp.path().join("skills");
        std::fs::create_dir_all(skills.join("existing")).unwrap();
        std::fs::write(skills.join("existing/SKILL.md"), "mine").unwrap();
        let ledger = files(&[
            ("README.md", "# x"),
            ("bots/matt-ag1/bot.json", r#"{"id":"ag_1","name":"Mattress Matt","is_bot":1,"config":{"model":"x","api_key":"[left out: looks like a credential]"},"unknown_column":5}"#),
            ("bots/matt-ag1/workspace/SOUL.md", "You sell mattresses."),
            ("routines/daily-rt1.json", r#"{"id":"rt_1","name":"Daily","status":"active","schedule_expression":"0 9 * * *","next_run_at":"2026-10-17"}"#),
            ("workflows/mine.json", r#"{"id":"wf_mine","title":"Mine"}"#),
            ("workflows/theirs.json", r#"{"id":"wf_theirs","title":"Theirs"}"#),
            ("skills/brand-voice/SKILL.md", "Write plainly."),
            ("skills/existing/SKILL.md", "from ledger"),
            ("connected-apps.json", r#"[{"connector":"gmail","account":"eoj@example.com","status":"active"}]"#),
            ("projects/broken.json", "{not json"),
        ]);

        let preview = restore(&conn, "u1", &ledger, Some(&skills), |id| ws.join(id), false).unwrap();
        let act = |p: &str| preview.items.iter().find(|i| i.path == p).map(|i| i.action).unwrap();
        assert_eq!(act("bots/matt-ag1/bot.json"), "add");
        assert_eq!(act("routines/daily-rt1.json"), "add");
        assert_eq!(act("workflows/mine.json"), "already_here");
        assert_eq!(act("workflows/theirs.json"), "skip");
        assert_eq!(act("projects/broken.json"), "skip");
        let bot = preview.items.iter().find(|i| i.path == "bots/matt-ag1/bot.json").unwrap();
        assert_eq!(bot.needs, vec!["config.api_key"]);
        assert!(preview.items.iter().find(|i| i.path == "routines/daily-rt1.json").unwrap().paused);
        assert_eq!((preview.files_added, preview.files_kept), (2, 1));
        assert_eq!(preview.reconnect, vec![serde_json::json!({"connector":"gmail","account":"eoj@example.com"})]);
        // A preview changes nothing.
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM agents", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(!ws.join("ag_1/SOUL.md").exists());

        restore(&conn, "u1", &ledger, Some(&skills), |id| ws.join(id), true).unwrap();
        let (owner, config): (String, String) = conn.query_row("SELECT user_id, config FROM agents WHERE id='ag_1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(owner, "u1");
        assert!(config.contains("\"model\"") && !config.contains("api_key") && !config.contains("left out"));
        let (status, next): (String, Option<String>) = conn.query_row("SELECT status, next_run_at FROM routines WHERE id='rt_1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((status.as_str(), next), ("paused", None));
        assert_eq!(std::fs::read_to_string(ws.join("ag_1/SOUL.md")).unwrap(), "You sell mattresses.");
        assert_eq!(std::fs::read_to_string(skills.join("brand-voice/SKILL.md")).unwrap(), "Write plainly.");
        assert_eq!(std::fs::read_to_string(skills.join("existing/SKILL.md")).unwrap(), "mine", "never overwrites");

        // Running it again restores nothing new.
        let again = restore(&conn, "u1", &ledger, Some(&skills), |id| ws.join(id), false).unwrap();
        assert!(again.items.iter().filter(|i| i.path.starts_with("bots/") || i.path.starts_with("routines/")).all(|i| i.action == "already_here"));
        assert_eq!(again.files_added, 0);
    }

    #[test]
    fn unsafe_paths_and_ids_are_ignored() {
        let conn = db();
        let tmp = tempfile::tempdir().unwrap();
        let ledger = files(&[
            ("../evil.json", r#"{"id":"x"}"#),
            ("bots/b/bot.json", r#"{"id":"../../etc","name":"Bad"}"#),
            ("bots/b/workspace/x.md", "hi"),
        ]);
        let plan = restore(&conn, "u1", &ledger, None, |id| tmp.path().join(id), true).unwrap();
        assert_eq!(plan.files_added, 0);
        assert!(!tmp.path().join("../etc/x.md").exists());
    }
}
