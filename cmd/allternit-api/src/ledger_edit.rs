//! Edit a ledger file and the real thing changes: ledger program phase 4,
//! step 3 (files as the source of truth for edits).
//!
//! - A record file (`bots/<x>/bot.json`, `routines/<x>.json`, …) updates that
//!   bot, routine or other record: only columns it already has, never its id,
//!   owner or runtime state (the ledger's own drop list).
//! - `[left out: …]` values keep what's stored; credentials typed into a
//!   record are refused (set them in the app, never in the ledger).
//! - A bot workspace file or a file in an installed skill is written where
//!   the bot or skill reads it.
//! - `README.md` and `connected-apps.json` are written by Allternit and
//!   can't be edited.
//!
//! Creating new things and deleting things through the ledger aren't
//! supported yet; edits only.
use std::path::{Path, PathBuf};

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use serde_json::{Map, Value};

use crate::ledger::{Kind, KINDS, LEFT_OUT};
use crate::ledger_repo::LedgerRepo;
use crate::memory_drive_service::{Result, ServiceError};

pub const MAX_EDIT_BYTES: usize = 1024 * 1024;
const LEFT_OUT_PREFIX: &str = "[left out";

fn say(msg: impl Into<String>) -> ServiceError {
    ServiceError::Provenance(msg.into())
}

/// The kind a record file belongs to.
pub fn kind_for(path: &str) -> Option<&'static Kind> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["bots", _, "bot.json"] => return KINDS.iter().find(|k| k.dir == "bots"),
        ["agents", _, "agent.json"] => return KINDS.iter().find(|k| k.dir == "agents"),
        _ => {}
    }
    if !path.ends_with(".json") {
        return None;
    }
    KINDS
        .iter()
        .filter(|k| k.dir != "bots" && k.dir != "agents")
        .filter(|k| path.strip_prefix(k.dir).and_then(|r| r.strip_prefix('/')).is_some_and(|r| !r.contains('/')))
        .max_by_key(|k| k.dir.len())
}

fn is_left_out(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.starts_with(LEFT_OUT_PREFIX))
}

/// Put back stored values wherever the edit still says "[left out…]".
fn keep_left_out(edited: &mut Value, stored: Option<&Value>) {
    match edited {
        Value::Object(o) => {
            let keys: Vec<String> = o.keys().cloned().collect();
            for k in keys {
                let old = stored.and_then(|s| s.get(&k));
                if is_left_out(&o[&k]) {
                    match old {
                        Some(v) => {
                            o.insert(k, v.clone());
                        }
                        None => {
                            o.remove(&k);
                        }
                    }
                } else if let Some(child) = o.get_mut(&k) {
                    keep_left_out(child, old);
                }
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter_mut().enumerate() {
                keep_left_out(x, stored.and_then(|s| s.get(i)));
            }
        }
        _ => {}
    }
}

/// A credential typed into the ledger (not the left-out marker).
fn typed_credential(v: &Value) -> Option<String> {
    match v {
        Value::Object(o) => o.iter().find_map(|(k, x)| {
            if crate::ledger::credential_key(k) && x.as_str().is_some_and(|s| !s.is_empty() && !s.starts_with(LEFT_OUT_PREFIX)) {
                Some(k.clone())
            } else {
                typed_credential(x)
            }
        }),
        Value::Array(a) => a.iter().find_map(typed_credential),
        _ => None,
    }
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

fn stored_row(conn: &Connection, kind: &Kind, owner: &str, id: &str) -> Result<Option<Map<String, Value>>> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {} WHERE {} = ?1 AND user_id = ?2", kind.table, kind.id))?;
    let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![id, owner])?;
    let Some(r) = rows.next()? else { return Ok(None) };
    let mut m = Map::new();
    for (i, c) in cols.iter().enumerate() {
        m.insert(c.clone(), crate::ledger::cell(r.get_ref(i)?));
    }
    Ok(Some(m))
}

/// Columns an edit may never change.
fn fixed(kind: &Kind, col: &str) -> bool {
    matches!(col, "id" | "user_id" | "created_at" | "updated_at") || col == kind.id || kind.drop.contains(&col)
}

fn edit_record(conn: &Connection, owner: &str, repo: &LedgerRepo, kind: &Kind, path: &str, content: &str) -> Result<String> {
    let current = repo
        .read(path, None)
        .map_err(|e| say(e.to_string()))?
        .ok_or_else(|| say("That file isn't in your ledger. Make new things in Allternit, then edit them here."))?;
    let current: Value = serde_json::from_slice(&current).map_err(|_| say("The saved file isn't readable; save the ledger again first."))?;
    let mut edited: Value = serde_json::from_str(content).map_err(|e| say(format!("That isn't valid JSON: {e}.")))?;
    if !edited.is_object() {
        return Err(say("A record file must be one JSON object."));
    }
    let id = current.get(kind.id).and_then(|v| v.as_str()).ok_or_else(|| say("This file has no id."))?.to_string();
    if edited.get(kind.id).and_then(|v| v.as_str()) != Some(id.as_str()) {
        return Err(say("The id can't change."));
    }
    if let Some(key) = typed_credential(&edited) {
        return Err(say(format!("Keep keys and passwords out of the ledger ({key}). Set them in Allternit's settings for this {}.", kind.dir.trim_end_matches('s'))));
    }
    let stored = stored_row(conn, kind, owner, &id)?.ok_or_else(|| say("That isn't in this account any more."))?;
    keep_left_out(&mut edited, Some(&Value::Object(stored.clone())));
    let Value::Object(edited) = edited else { unreachable!() };

    let mut sets = Vec::new();
    let mut params: Vec<SqlValue> = Vec::new();
    let mut changed = Vec::new();
    for (col, val) in &edited {
        if fixed(kind, col) || !stored.contains_key(col) {
            continue;
        }
        if stored.get(col) != Some(val) {
            sets.push(format!("{col} = ?"));
            params.push(to_sql(val));
            changed.push(col.clone());
        }
    }
    if sets.is_empty() {
        return Ok("No changes.".into());
    }
    if stored.contains_key("updated_at") {
        sets.push("updated_at = CURRENT_TIMESTAMP".into());
    }
    params.push(SqlValue::Text(id));
    params.push(SqlValue::Text(owner.into()));
    conn.execute(&format!("UPDATE {} SET {} WHERE {} = ? AND user_id = ?", kind.table, sets.join(", "), kind.id), params_from_iter(params))?;
    Ok(format!("Changed {}.", changed.join(", ")))
}

fn check_text(content: &str) -> Result<()> {
    if crate::ledger::token_like(content) || crate::ledger::assigns_secret(content) {
        return Err(say("That looks like it contains a key or password. Keep those out of the ledger."));
    }
    Ok(())
}

fn write_file(target: &Path, content: &str) -> Result<()> {
    if std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(say("That file is a link; edit it where it points."));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| say(format!("Couldn't save: {e}")))?;
    }
    std::fs::write(target, content).map_err(|e| say(format!("Couldn't save: {e}")))
}

/// Apply an edit of `path` to the real thing. Returns a short summary.
pub fn edit(
    conn: &Connection,
    owner: &str,
    repo: &LedgerRepo,
    path: &str,
    content: &str,
    skills_root: Option<&Path>,
    bot_workspace: impl Fn(&str) -> PathBuf,
) -> Result<String> {
    if !crate::ledger_repo::valid_path(path) {
        return Err(say("That isn't a ledger path."));
    }
    if content.len() > MAX_EDIT_BYTES {
        return Err(say("That file is too large to edit here (1 MB)."));
    }
    if matches!(path, "README.md" | "connected-apps.json") {
        return Err(say("Allternit writes this file. Change connected apps in Settings → Connected apps."));
    }
    if let Some(kind) = kind_for(path) {
        return edit_record(conn, owner, repo, kind, path, content);
    }
    let parts: Vec<&str> = path.splitn(4, '/').collect();
    match parts.as_slice() {
        [dir @ ("bots" | "agents"), stem, "workspace", rel] => {
            check_text(content)?;
            let record = format!("{dir}/{stem}/{}.json", if *dir == "bots" { "bot" } else { "agent" });
            let id = repo
                .read(&record, None)
                .map_err(|e| say(e.to_string()))?
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|v| v.get("id").and_then(|x| x.as_str()).map(str::to_string))
                .ok_or_else(|| say("That bot isn't in your ledger."))?;
            let kind = KINDS.iter().find(|k| k.dir == *dir).expect("bots and agents kinds");
            if stored_row(conn, kind, owner, &id)?.is_none() || !crate::ledger_repo::valid_path(&id) {
                return Err(say("That bot isn't in this account any more."));
            }
            write_file(&bot_workspace(&id).join(rel), content)?;
            Ok(format!("Saved {rel} for this bot."))
        }
        ["skills", name, ..] if *name != "recipes" && parts.len() >= 3 => {
            check_text(content)?;
            let root = skills_root.ok_or_else(|| say("This computer has no skills folder."))?;
            let folder = root.join(crate::ledger::slug(name));
            if !folder.join("SKILL.md").is_file() {
                return Err(say("Edit files of skills you've installed. Install new skills in Customize → Skills."));
            }
            let rel = path.splitn(3, '/').nth(2).unwrap_or_default();
            write_file(&folder.join(rel), content)?;
            Ok(format!("Saved {rel} in the {name} skill."))
        }
        _ => Err(say("Only bots, routines, workflows, templates, tasks, projects, schedules, skills and bot files can be edited.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, tempfile::TempDir, LedgerRepo) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (id TEXT PRIMARY KEY, user_id TEXT, name TEXT, config TEXT, system_prompt TEXT, is_bot INTEGER, archived_at TEXT, status TEXT, identity_key TEXT, updated_at TEXT);
             INSERT INTO agents VALUES ('ag_1','u1','Matt','{\"model\":\"x\",\"api_key\":\"sk-real\"}','Sell beds',1,NULL,'idle','secret-identity','2026-01-01');
             CREATE TABLE routines (id TEXT PRIMARY KEY, user_id TEXT, name TEXT, status TEXT, schedule_expression TEXT, next_run_at TEXT);
             INSERT INTO routines VALUES ('rt_1','u1','Daily','paused','0 9 * * *',NULL);
             INSERT INTO routines VALUES ('rt_2','u2','Theirs','active','0 9 * * *',NULL);",
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let repo = LedgerRepo::open(tmp.path().join("ledger.git")).unwrap();
        let skills = tmp.path().join("skills");
        std::fs::create_dir_all(skills.join("brand-voice")).unwrap();
        std::fs::write(skills.join("brand-voice/SKILL.md"), "old").unwrap();
        let c = crate::ledger::collect(&conn, "u1", &[skills], |id| tmp.path().join("ws").join(id));
        repo.write_snapshot(&c.files, "Save").unwrap();
        (conn, tmp, repo)
    }

    fn file(repo: &LedgerRepo, p: &str) -> Value {
        serde_json::from_slice(&repo.read(p, None).unwrap().unwrap()).unwrap()
    }

    #[test]
    fn editing_a_record_changes_the_real_thing_and_keeps_left_out_values() {
        let (conn, tmp, repo) = setup();
        let ws = |id: &str| tmp.path().join("ws").join(id);
        let skills = tmp.path().join("skills");
        let mut bot = file(&repo, "bots/matt-ag1/bot.json");
        assert_eq!(bot["config"]["api_key"], LEFT_OUT);
        bot["name"] = "Mattress Matt".into();
        bot["system_prompt"] = "Sell beds kindly".into();
        bot["config"]["model"] = "y".into();
        let out = edit(&conn, "u1", &repo, "bots/matt-ag1/bot.json", &bot.to_string(), Some(&skills), ws).unwrap();
        assert!(out.starts_with("Changed"), "{out}");
        let (name, prompt, config, identity): (String, String, String, String) = conn
            .query_row("SELECT name, system_prompt, config, identity_key FROM agents WHERE id='ag_1'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        assert_eq!((name.as_str(), prompt.as_str()), ("Mattress Matt", "Sell beds kindly"));
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["model"], "y");
        assert_eq!(config["api_key"], "sk-real", "left-out values keep what's stored");
        assert_eq!(identity, "secret-identity");

        // Turning a routine back on through its file.
        let mut rt = file(&repo, "routines/daily-rt1.json");
        rt["status"] = "active".into();
        edit(&conn, "u1", &repo, "routines/daily-rt1.json", &rt.to_string(), None, ws).unwrap();
        assert_eq!(conn.query_row("SELECT status FROM routines WHERE id='rt_1'", [], |r| r.get::<_, String>(0)).unwrap(), "active");
        assert_eq!(edit(&conn, "u1", &repo, "routines/daily-rt1.json", &rt.to_string(), None, ws).unwrap(), "No changes.");
    }

    #[test]
    fn refuses_unsafe_edits() {
        let (conn, tmp, repo) = setup();
        let ws = |id: &str| tmp.path().join("ws").join(id);
        let bot = file(&repo, "bots/matt-ag1/bot.json");
        let mut changed_id = bot.clone();
        changed_id["id"] = "rt_2".into();
        assert!(edit(&conn, "u1", &repo, "bots/matt-ag1/bot.json", &changed_id.to_string(), None, ws).is_err());
        let mut typed_key = bot.clone();
        typed_key["config"]["api_key"] = "sk-typedintheledger".into();
        let err = edit(&conn, "u1", &repo, "bots/matt-ag1/bot.json", &typed_key.to_string(), None, ws).unwrap_err().to_string();
        assert!(err.contains("Keep keys and passwords out of the ledger"), "{err}");
        assert!(edit(&conn, "u1", &repo, "bots/matt-ag1/bot.json", "{not json", None, ws).is_err());
        assert!(edit(&conn, "u1", &repo, "README.md", "x", None, ws).is_err());
        assert!(edit(&conn, "u1", &repo, "routines/new.json", "{}", None, ws).is_err());
        assert!(edit(&conn, "u1", &repo, "../x", "x", None, ws).is_err());
        assert!(edit(&conn, "u1", &repo, "bots/matt-ag1/workspace/SOUL.md", "API_TOKEN=abcdef123456", None, ws).is_err());
    }

    #[test]
    fn edits_bot_files_and_installed_skills() {
        let (conn, tmp, repo) = setup();
        let ws = |id: &str| tmp.path().join("ws").join(id);
        let skills = tmp.path().join("skills");
        edit(&conn, "u1", &repo, "bots/matt-ag1/workspace/SOUL.md", "You sell mattresses.", Some(&skills), ws).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.path().join("ws/ag_1/SOUL.md")).unwrap(), "You sell mattresses.");
        edit(&conn, "u1", &repo, "skills/brand-voice/SKILL.md", "new", Some(&skills), ws).unwrap();
        assert_eq!(std::fs::read_to_string(skills.join("brand-voice/SKILL.md")).unwrap(), "new");
        assert!(edit(&conn, "u1", &repo, "skills/not-installed/SKILL.md", "x", Some(&skills), ws).is_err());
    }
}
