//! `GET /terminal/list`: every Allternit terminal with a title, cwd, command,
//! running state and (optionally) the last rows of its visible screen.
//!
//! The pane engine is the source of truth for which terminals exist
//! (`factory.terminal.list`). Its terminal info has no title, cwd, command or
//! creation time, so allternit-api keeps a small in-memory record of what it
//! was asked to create ([`TerminalRecords`]) and reads the live title and cwd
//! from the terminal's pane (`pane.get`) and the screen from `pane.read`.
//! Terminals that were not created through this API still appear, with what
//! the engine gives.
//!
//! A failure on one terminal's pane calls drops only that terminal's extra
//! fields; only a failure to list the terminals at all fails the request.

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::terminal_routes::{pane_err, TerminalSessionStore};

/// What allternit-api itself knows about a terminal it created.
#[derive(Debug, Clone)]
pub(crate) struct TerminalRecord {
    pub title: Option<String>,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// Created with a `command` (not a shell): a graceful close may SIGTERM it.
    pub custom_command: bool,
}

pub(crate) type TerminalRecords = Arc<Mutex<HashMap<String, TerminalRecord>>>;

/// Most rows a preview returns.
const MAX_PREVIEW_ROWS: u32 = 40;
/// Per-terminal budget for the pane calls (`pane.get` + `pane.read`).
const PANE_CALL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    #[serde(default)]
    pub preview: Option<u32>,
}

/// Env keys the engine can pass to a child: non-empty, no `=` and no NUL.
pub(crate) fn validate_env(env: &HashMap<String, String>) -> Result<(), String> {
    for key in env.keys() {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return Err(format!(
                "invalid env key {key:?}: keys must be non-empty and contain no '=' or NUL"
            ));
        }
    }
    Ok(())
}

/// A title the user set, else the terminal's own title, else the cwd's last
/// folder, else "Terminal". The engine's default workspace label
/// (`term-<id>`) is not a title.
pub(crate) fn resolve_title(
    created: Option<&str>,
    pane_title: Option<&str>,
    cwd: Option<&str>,
) -> String {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(t) = clean(created) {
        return t.to_string();
    }
    if let Some(t) = clean(pane_title).filter(|t| !t.starts_with("term-")) {
        return t.to_string();
    }
    clean(cwd)
        .and_then(|c| {
            std::path::Path::new(c.trim_end_matches(['/', '\\']))
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Terminal".to_string())
}

/// The last `n` rows of a screen dump.
pub(crate) fn last_rows(text: &str, n: usize) -> Vec<String> {
    let rows: Vec<&str> = text.split_terminator('\n').collect();
    let start = rows.len().saturating_sub(n);
    rows[start..]
        .iter()
        .map(|r| r.trim_end_matches('\r').to_string())
        .collect()
}

/// Live facts from the terminal's pane.
struct PaneFacts {
    title: Option<String>,
    cwd: Option<String>,
    preview: Option<Vec<String>>,
}

async fn pane_facts(
    store: &TerminalSessionStore,
    pane_id: &str,
    preview: Option<usize>,
) -> PaneFacts {
    let get = async {
        store
            .request("pane.get", json!({ "pane_id": pane_id }))
            .await
            .ok()
    };
    let read = async {
        let rows = preview?;
        let result = store
            .request(
                "pane.read",
                json!({ "pane_id": pane_id, "source": "visible", "lines": rows, "strip_ansi": true }),
            )
            .await
            .ok()?;
        let text = result["read"]["text"].as_str()?.to_string();
        Some(last_rows(&text, rows))
    };
    let both = async { tokio::join!(get, read) };
    let Ok((got, preview)) = tokio::time::timeout(PANE_CALL_TIMEOUT, both).await else {
        return PaneFacts {
            title: None,
            cwd: None,
            preview: None,
        };
    };
    static NULL: Value = Value::Null;
    let pane = got.as_ref().map(|r| &r["pane"]).unwrap_or(&NULL);
    let text = |key: &str| pane[key].as_str().map(str::to_string);
    PaneFacts {
        title: text("terminal_title"),
        cwd: text("foreground_cwd").or_else(|| text("cwd")),
        preview,
    }
}

pub(crate) async fn list_terminals(
    State(store): State<TerminalSessionStore>,
    Query(query): Query<ListQuery>,
) -> Response {
    let listed = match store.request("factory.terminal.list", json!({})).await {
        Ok(v) => v,
        Err(err) => return pane_err(err).into_response(),
    };
    let engine: Vec<Value> = listed["terminals"].as_array().cloned().unwrap_or_default();
    let preview = query.preview.map(|n| n.clamp(1, MAX_PREVIEW_ROWS) as usize);

    // Forget records of terminals the engine no longer has.
    let live: HashSet<&str> = engine
        .iter()
        .filter_map(|t| t["terminal_id"].as_str())
        .collect();
    let records: HashMap<String, TerminalRecord> = {
        let mut map = store.records.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|id, _| live.contains(id.as_str()));
        map.clone()
    };

    let entries = engine.iter().map(|info| {
        let store = &store;
        let records = &records;
        async move {
            let id = info["terminal_id"].as_str().unwrap_or_default().to_string();
            let record = records.get(&id);
            let facts = match info["pane_id"].as_str() {
                Some(pane_id) => Some(pane_facts(store, pane_id, preview).await),
                None => None,
            };
            let cwd = facts
                .as_ref()
                .and_then(|f| f.cwd.clone())
                .or_else(|| record.and_then(|r| r.cwd.clone()));
            let title = resolve_title(
                record.and_then(|r| r.title.as_deref()),
                facts.as_ref().and_then(|f| f.title.as_deref()),
                cwd.as_deref(),
            );
            let mut entry = json!({
                "session_id": id,
                "pane_id": info["pane_id"],
                "title": title,
                "cwd": cwd,
                "command": record.map(|r| r.command.clone()).unwrap_or_default(),
                "running": info["running"].as_bool().unwrap_or(false),
                "exit_code": info["exit_code"],
                "pid": info["pid"],
                "created_at": record.map(|r| r.created_at.clone()),
            });
            if let Some(rows) = facts.and_then(|f| f.preview) {
                entry["preview"] = json!(rows);
            }
            entry
        }
    });
    let terminals = futures::future::join_all(entries).await;
    Json(json!({ "terminals": terminals })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_prefers_created_then_pane_then_cwd() {
        assert_eq!(
            resolve_title(Some(" probe "), Some("zsh"), Some("/a/b")),
            "probe"
        );
        assert_eq!(resolve_title(None, Some("vim"), Some("/a/b")), "vim");
        assert_eq!(
            resolve_title(None, Some("term-abc"), Some("/a/proj/")),
            "proj"
        );
        assert_eq!(resolve_title(None, Some(""), None), "Terminal");
    }

    #[test]
    fn preview_keeps_the_last_rows_including_blank_ones() {
        assert_eq!(last_rows("a\r\nb\n\nc\n", 3), vec!["b", "", "c"]);
        assert_eq!(last_rows("a\n", 5), vec!["a"]);
    }
}
