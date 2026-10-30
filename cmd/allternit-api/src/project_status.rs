//! Project status pages: one living status object per project, stored in
//! `cowork_projects.metadata.status` (no migration). Bots read it before work
//! and update it after; the routes in `cowork_routes.rs` and the prompt
//! builders in `coordinator_routes.rs` both go through this module.

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::netops::alerts;

pub const SECTIONS: [&str; 9] = [
    "goal",
    "status",
    "next_actions",
    "owners",
    "deadlines",
    "decisions",
    "blockers",
    "outcome",
    "lessons",
];
const STATES: [&str; 5] = ["on_track", "at_risk", "blocked", "needs_decision", "done"];
const LOG_MAX: usize = 10;
const ARCHIVE_MAX: usize = 200;
/// Attention kinds this module opens; each is one open item per project+state.
const ATTENTION_KEYS: [&str; 3] = ["blocked", "needs_decision", "overdue"];

#[derive(Debug)]
pub enum StatusError {
    NotFound,
    ProofRequired,
    Closed,
    BadRequest(String),
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for StatusError {
    fn from(e: rusqlite::Error) -> Self {
        StatusError::Db(e)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchBody {
    pub by: Option<String>,
    pub section: Option<String>,
    pub text: Option<String>,
    pub state: Option<String>,
    pub next_action: Option<String>,
    pub note: Option<String>,
    pub proof_url: Option<String>,
    pub done: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CloseBody {
    pub by: Option<String>,
    pub outcome: Option<String>,
    pub lessons: Option<String>,
    pub reopen: Option<bool>,
}

fn empty_section() -> Value {
    json!({"text": "", "updated_by": "", "updated_at": ""})
}

/// The status object with every default filled in, whatever was stored.
pub fn normalize(stored: Option<&Value>) -> Value {
    let mut out = Map::new();
    let src = stored.and_then(|v| v.as_object());
    let get = |k: &str| src.and_then(|m| m.get(k));
    out.insert("version".into(), json!(1));
    let lifecycle = match get("lifecycle").and_then(|v| v.as_str()) {
        Some("closed") => "closed",
        _ => "active",
    };
    out.insert("lifecycle".into(), json!(lifecycle));
    let state = get("state")
        .and_then(|v| v.as_str())
        .filter(|s| STATES.contains(s))
        .unwrap_or("on_track");
    out.insert("state".into(), json!(state));
    out.insert(
        "next_action".into(),
        json!(get("next_action").and_then(|v| v.as_str()).unwrap_or("")),
    );
    let mut sections = Map::new();
    let stored_sections = get("sections").and_then(|v| v.as_object());
    for key in SECTIONS {
        let sec = stored_sections
            .and_then(|m| m.get(key))
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(empty_section);
        sections.insert(key.to_string(), sec);
    }
    out.insert("sections".into(), Value::Object(sections));
    for key in ["log", "archive"] {
        let arr = get(key).filter(|v| v.is_array()).cloned().unwrap_or_else(|| json!([]));
        out.insert(key.into(), arr);
    }
    out.insert("revision".into(), json!(get("revision").and_then(|v| v.as_i64()).unwrap_or(0)));
    out.insert("closed_at".into(), get("closed_at").cloned().unwrap_or(Value::Null));
    Value::Object(out)
}

fn parse_metadata(raw: Option<String>) -> Map<String, Value> {
    raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

fn status_of(meta: &Map<String, Value>) -> Value {
    normalize(meta.get("status"))
}

fn push_log(status: &mut Value, entry: Value) {
    let log = status["log"].as_array_mut().expect("normalized log");
    log.insert(0, entry);
    let overflow: Vec<Value> = if log.len() > LOG_MAX { log.split_off(LOG_MAX) } else { Vec::new() };
    if !overflow.is_empty() {
        let archive = status["archive"].as_array_mut().expect("normalized archive");
        // The overflow is the oldest of the log yet newer than the archive.
        for (i, e) in overflow.into_iter().enumerate() {
            archive.insert(i, e);
        }
        archive.truncate(ARCHIVE_MAX);
    }
}

fn set_section(status: &mut Value, key: &str, text: &str, by: &str, at: &str) {
    status["sections"][key] = json!({"text": text, "updated_by": by, "updated_at": at});
}

/// Run `f` on the project's metadata + normalized status inside one write
/// transaction, then store the metadata back. Two bots writing different
/// sections serialize here instead of clobbering each other.
fn with_status<T>(
    conn: &Connection,
    id: &str,
    user_id: &str,
    f: impl FnOnce(&mut Value) -> Result<T, StatusError>,
) -> Result<(Value, T), StatusError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let run = (|| {
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT metadata FROM cowork_projects WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
                |r| r.get(0),
            )
            .optional()?;
        let raw = raw.ok_or(StatusError::NotFound)?;
        let mut meta = parse_metadata(raw);
        let mut status = status_of(&meta);
        let out = f(&mut status)?;
        meta.insert("status".into(), status.clone());
        conn.execute(
            "UPDATE cowork_projects SET metadata = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND user_id = ?3",
            params![Value::Object(meta).to_string(), id, user_id],
        )?;
        Ok::<_, StatusError>((status, out))
    })();
    match run {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

pub fn get_status(conn: &Connection, id: &str, user_id: &str) -> Result<Value, StatusError> {
    let raw: Option<Option<String>> = conn
        .query_row(
            "SELECT metadata FROM cowork_projects WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            |r| r.get(0),
        )
        .optional()?;
    let meta = parse_metadata(raw.ok_or(StatusError::NotFound)?);
    Ok(status_of(&meta))
}

pub fn patch_status(
    conn: &Connection,
    id: &str,
    user_id: &str,
    body: &PatchBody,
) -> Result<Value, StatusError> {
    let by = body.by.clone().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "you".into());
    let proof = body.proof_url.clone().filter(|p| !p.trim().is_empty());
    let wants_done = body.done == Some(true) || body.state.as_deref() == Some("done");
    if let Some(s) = body.state.as_deref() {
        if !STATES.contains(&s) {
            return Err(StatusError::BadRequest(format!("unknown state: {s}")));
        }
    }
    if let Some(sec) = body.section.as_deref() {
        if !SECTIONS.contains(&sec) {
            return Err(StatusError::BadRequest(format!("unknown section: {sec}")));
        }
    }
    if body.text.is_some() && body.section.is_none() {
        return Err(StatusError::BadRequest("text needs a section".into()));
    }
    if wants_done && proof.is_none() {
        return Err(StatusError::ProofRequired);
    }
    let (status, _) = with_status(conn, id, user_id, |status| {
        if status["lifecycle"] == "closed" {
            return Err(StatusError::Closed);
        }
        let now = Utc::now().to_rfc3339();
        if let (Some(sec), Some(text)) = (body.section.as_deref(), body.text.as_deref()) {
            set_section(status, sec, text, &by, &now);
        }
        if wants_done {
            status["state"] = json!("done");
        } else if let Some(s) = body.state.as_deref() {
            status["state"] = json!(s);
        }
        if let Some(n) = body.next_action.as_deref() {
            status["next_action"] = json!(n);
        }
        let note = body
            .note
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| match body.section.as_deref() {
                Some(sec) => format!("updated {sec}"),
                None => "updated status".to_string(),
            });
        push_log(
            status,
            json!({
                "at": now,
                "by": by,
                "section": body.section,
                "note": note,
                "proof_url": proof,
            }),
        );
        let rev = status["revision"].as_i64().unwrap_or(0) + 1;
        status["revision"] = json!(rev);
        Ok(())
    })?;
    sync_attention(conn, id, user_id, &status);
    Ok(status)
}

/// Exception hook: blocked / needs_decision / overdue open one attention item
/// per project+state (id `project_status:<project>:<key>`); leaving the state
/// resolves it. Best effort: a failure here never fails the status write.
fn sync_attention(conn: &Connection, id: &str, user_id: &str, status: &Value) {
    let state = status["state"].as_str().unwrap_or("on_track");
    let last_note = status["log"][0]["note"].as_str().unwrap_or("");
    let active = match state {
        "blocked" => Some("blocked"),
        "needs_decision" => Some("needs_decision"),
        "at_risk" if last_note.trim_start().to_lowercase().starts_with("overdue") => Some("overdue"),
        _ => None,
    };
    for key in ATTENTION_KEYS {
        if Some(key) != active {
            alerts::resolve(conn, user_id, &format!("project_status:{id}:{key}"), "fixed");
        }
    }
    let Some(key) = active else { return };
    let title: String = conn
        .query_row(
            "SELECT title FROM cowork_projects WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "A project".into());
    let next = status["next_action"].as_str().unwrap_or("");
    let what = match key {
        "blocked" => "is blocked",
        "needs_decision" => "needs a decision",
        _ => "is overdue",
    };
    let mut summary = format!("{title} {what}");
    if !next.is_empty() {
        summary.push_str(&format!(". Next: {next}"));
    }
    alerts::open(
        conn,
        &alerts::Item {
            owner: user_id.to_string(),
            kind: "project_status".into(),
            severity: "warn".into(),
            source: "project".into(),
            source_id: format!("{id}:{key}"),
            summary,
            detail: json!({"projectId": id, "state": state, "note": last_note}),
            ..Default::default()
        },
    );
}

/// Close (or reopen) a project. Closing pauses routines linked to it.
/// Returns the status and how many routines were paused.
pub fn close_project(
    conn: &Connection,
    id: &str,
    user_id: &str,
    body: &CloseBody,
) -> Result<(Value, usize), StatusError> {
    let by = body.by.clone().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "you".into());
    let reopen = body.reopen == Some(true);
    let (status, ()) = with_status(conn, id, user_id, |status| {
        let now = Utc::now().to_rfc3339();
        let closed = status["lifecycle"] == "closed";
        if reopen {
            status["lifecycle"] = json!("active");
            status["closed_at"] = Value::Null;
            if status["state"] == "done" {
                status["state"] = json!("on_track");
            }
        } else {
            if closed {
                return Err(StatusError::Closed);
            }
            if let Some(o) = body.outcome.as_deref() {
                set_section(status, "outcome", o, &by, &now);
            }
            if let Some(l) = body.lessons.as_deref() {
                set_section(status, "lessons", l, &by, &now);
            }
            status["lifecycle"] = json!("closed");
            status["closed_at"] = json!(now);
        }
        let section = if reopen { Value::Null } else { json!("outcome") };
        let note = if reopen { "reopened project" } else { "closed project" };
        push_log(
            status,
            json!({"at": now, "by": by, "section": section, "note": note, "proof_url": Value::Null}),
        );
        let rev = status["revision"].as_i64().unwrap_or(0) + 1;
        status["revision"] = json!(rev);
        Ok(())
    })?;
    let mut paused = 0usize;
    if !reopen {
        paused += conn.execute(
            "UPDATE routines SET status = 'paused', updated_at = datetime('now')
             WHERE user_id = ?1 AND status != 'paused'
               AND (workspace_id = ?2 OR CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.projectId') END = ?2)",
            params![user_id, id],
        )?;
        paused += conn.execute(
            "UPDATE cowork_routines SET enabled = 0
             WHERE user_id = ?1 AND enabled = 1 AND workspace = ?2",
            params![user_id, id],
        )?;
        for key in ATTENTION_KEYS {
            alerts::resolve(conn, user_id, &format!("project_status:{id}:{key}"), "dismissed");
        }
    }
    Ok((status, paused))
}

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max).collect::<String>())
    }
}

/// The compact block bots read before they start.
pub fn render_block(status: &Value) -> String {
    let status = normalize(Some(status));
    let mut out = String::from(
        "## Project status page (read before you start; update it with project_status when you finish)\n",
    );
    let next = status["next_action"].as_str().unwrap_or("");
    out.push_str(&format!(
        "State: {} · Next: {}\n",
        status["state"].as_str().unwrap_or("on_track"),
        if next.is_empty() { "(none set)" } else { next }
    ));
    let labels = [
        ("goal", "Goal"),
        ("status", "Status"),
        ("next_actions", "Next actions"),
        ("owners", "Owners"),
        ("deadlines", "Deadlines"),
        ("decisions", "Decisions"),
        ("blockers", "Blockers"),
    ];
    let parts: Vec<String> = labels
        .iter()
        .filter_map(|(k, label)| {
            let t = status["sections"][*k]["text"].as_str().unwrap_or("").trim();
            (!t.is_empty()).then(|| format!("{label}: {}", clip(t, 400)))
        })
        .collect();
    if parts.is_empty() {
        out.push_str("(No status written yet. Fill it in when you finish.)\n");
    } else {
        out.push_str(&parts.join(" / "));
        out.push('\n');
    }
    let lines: Vec<String> = status["log"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(3)
                .map(|e| {
                    format!(
                        "- {} {}: {}",
                        e["at"].as_str().unwrap_or("").get(..10).unwrap_or(""),
                        e["by"].as_str().unwrap_or(""),
                        clip(e["note"].as_str().unwrap_or(""), 160)
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if !lines.is_empty() {
        out.push_str("Recent log:\n");
        out.push_str(&lines.join("\n"));
        out.push('\n');
    }
    out
}

/// Project instructions + status block for prompts. Empty when the project
/// is missing; never errors (a prompt must not fail on a missing page).
pub fn prompt_context(conn: &Connection, project_id: &str) -> String {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT instructions, metadata FROM cowork_projects WHERE id = ?1",
            params![project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap_or(None);
    let Some((instructions, metadata)) = row else { return String::new() };
    let mut out = String::new();
    if let Some(i) = instructions.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        out.push_str("## Project instructions\n");
        out.push_str(&clip_keep_lines(i, 4000));
        out.push_str("\n\n");
    }
    out.push_str(&render_block(&status_of(&parse_metadata(metadata))));
    out.push_str(&format!("Project id (pass as project_id to project_status): {project_id}\n"));
    out
}

fn clip_keep_lines(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
