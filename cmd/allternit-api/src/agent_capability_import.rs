//! Import an agent from another tool on this computer — Claude Code, Codex,
//! OpenClaw or Hermes — into Allternit (Customize › Add › Import an agent).
//!
//! `GET /agent-import/scan` reports, per tool found: its skills (folders with a
//! SKILL.md), its MCP servers, and its instructions file (the text, for the
//! app to put in a bot's SOUL.md after the person confirms). Secrets are never
//! read out: MCP `env` / `headers` values are dropped, only their names are
//! reported so the person knows what to fill in. Servers that run a local
//! command (stdio) are listed as not importable: Allternit's connectors are
//! web addresses.
//!
//! `POST /agent-import/skills` copies the chosen skill folders into the skills
//! root through the same versioned writer as catalog skills
//! (`skill_catalog_install::write_skill`). MCP servers go through the existing
//! `POST /mcp/connectors`, the instructions through the bot workspace file
//! API, and memory through `/memory/drive/import-agents` — all from the app.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

/// Largest instructions file returned, and most skills / servers per tool.
const MAX_INSTRUCTIONS_BYTES: u64 = 64 * 1024;
const MAX_ITEMS: usize = 200;

struct Tool {
    id: &'static str,
    name: &'static str,
}

const TOOLS: &[Tool] = &[
    Tool { id: "claude-code", name: "Claude Code" },
    Tool { id: "codex", name: "Codex" },
    Tool { id: "openclaw", name: "OpenClaw" },
    Tool { id: "hermes", name: "Hermes" },
];

fn home() -> Option<PathBuf> {
    crate::memory_drive_agent_import::home()
}

/// Folders that hold skills for each tool.
fn skill_roots(home: &Path, id: &str) -> Vec<PathBuf> {
    match id {
        "claude-code" => vec![home.join(".claude/skills")],
        "codex" => vec![std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex")).join("skills")],
        "openclaw" => vec![home.join(".openclaw/workspace/skills"), home.join(".openclaw/skills")],
        "hermes" => vec![home.join(".hermes/skills")],
        _ => vec![],
    }
}

fn instructions_file(home: &Path, id: &str) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match id {
        "claude-code" => vec![home.join(".claude/CLAUDE.md")],
        "codex" => vec![std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex")).join("AGENTS.md")],
        "openclaw" => vec![home.join(".openclaw/workspace/SOUL.md"), home.join(".openclaw/workspace/AGENTS.md")],
        "hermes" => vec![home.join(".hermes/SOUL.md")],
        _ => vec![],
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// Skill folders (any depth up to 3, for Hermes' category folders) under the roots.
fn find_skills(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
        if out.len() >= MAX_ITEMS || depth > 3 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if d.join("SKILL.md").is_file() {
                out.push((name, d));
            } else {
                walk(&d, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut out);
    }
    out
}

/// One MCP server as the scan reports it: never a secret value.
fn server(name: &str, url: Option<&str>, command: Option<&str>, secret_names: Vec<String>) -> Value {
    let url = url.map(str::trim).filter(|u| u.starts_with("http://") || u.starts_with("https://"));
    json!({
        "name": name,
        "url": url,
        "command": command.map(|c| c.rsplit('/').next().unwrap_or(c)),
        "importable": url.is_some(),
        "reason": if url.is_some() { Value::Null } else { json!("Runs a local command; Allternit connectors are web addresses.") },
        "secrets": secret_names,
    })
}

fn keys_of(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// Claude Code (`~/.claude.json` `mcpServers`) and Hermes (`~/.hermes/config.yaml` `mcp_servers`).
fn servers_from_map(map: Option<&Value>) -> Vec<Value> {
    let Some(map) = map.and_then(Value::as_object) else { return vec![] };
    map.iter()
        .take(MAX_ITEMS)
        .map(|(name, cfg)| {
            let mut secrets = keys_of(cfg.get("env"));
            secrets.extend(keys_of(cfg.get("headers")));
            server(name, cfg.get("url").and_then(Value::as_str), cfg.get("command").and_then(Value::as_str), secrets)
        })
        .collect()
}

/// Codex `config.toml`: `[mcp_servers.<name>]` tables with `url` / `command`;
/// `[mcp_servers.<name>.env]` sub-tables give secret names. A small line
/// reader (no TOML crate in this binary); anything it can't read is skipped.
pub(crate) fn codex_servers(text: &str) -> Vec<Value> {
    #[derive(Default)]
    struct S {
        url: Option<String>,
        command: Option<String>,
        secrets: Vec<String>,
    }
    let mut order: Vec<String> = Vec::new();
    let mut found: std::collections::HashMap<String, S> = std::collections::HashMap::new();
    let mut current: Option<(String, bool)> = None; // (server, in env/headers table)
    let unquote = |v: &str| v.trim().trim_matches('"').trim_matches('\'').to_string();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_matches(|c| c == '[' || c == ']').trim();
            current = header.strip_prefix("mcp_servers.").map(|rest| {
                let mut parts = rest.splitn(2, '.');
                let name = unquote(parts.next().unwrap_or(""));
                let sub = parts.next().map(|s| s == "env" || s == "headers" || s == "http_headers").unwrap_or(false);
                (name, sub)
            });
            if let Some((name, _)) = &current {
                if !found.contains_key(name) && !name.is_empty() {
                    order.push(name.clone());
                    found.insert(name.clone(), S::default());
                }
            }
            continue;
        }
        let Some((name, sub)) = &current else { continue };
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let Some(entry) = found.get_mut(name) else { continue };
        if *sub {
            entry.secrets.push(key.to_string());
        } else if key == "url" {
            entry.url = Some(unquote(value));
        } else if key == "command" {
            entry.command = Some(unquote(value));
        } else if key == "env" || key == "http_headers" {
            // Inline table: env = { KEY = "…" } → names only.
            for pair in value.trim_matches(|c| c == '{' || c == '}').split(',') {
                if let Some((k, _)) = pair.split_once('=') {
                    let k = k.trim().trim_matches('"');
                    if !k.is_empty() {
                        entry.secrets.push(k.to_string());
                    }
                }
            }
        }
    }
    order
        .into_iter()
        .take(MAX_ITEMS)
        .filter_map(|n| found.remove(&n).map(|s| server(&n, s.url.as_deref(), s.command.as_deref(), s.secrets)))
        .collect()
}

fn servers_for(home: &Path, id: &str) -> Vec<Value> {
    match id {
        "claude-code" => std::fs::read_to_string(home.join(".claude.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| servers_from_map(v.get("mcpServers")))
            .unwrap_or_default(),
        "codex" => std::fs::read_to_string(crate::vendor_local_connector::config_path("codex", home))
            .map(|t| codex_servers(&t))
            .unwrap_or_default(),
        "hermes" => std::fs::read_to_string(home.join(".hermes/config.yaml"))
            .ok()
            .and_then(|t| serde_yaml::from_str::<Value>(&t).ok())
            .map(|v| servers_from_map(v.get("mcp_servers")))
            .unwrap_or_default(),
        _ => vec![],
    }
}

pub(crate) fn scan_home(home: &Path) -> Vec<Value> {
    TOOLS
        .iter()
        .filter_map(|t| {
            let skills: Vec<Value> = find_skills(&skill_roots(home, t.id))
                .into_iter()
                .map(|(name, path)| json!({ "name": name, "path": path }))
                .collect();
            let servers = servers_for(home, t.id);
            let instructions = instructions_file(home, t.id).and_then(|p| {
                let size = std::fs::metadata(&p).ok()?.len();
                if size == 0 || size > MAX_INSTRUCTIONS_BYTES {
                    return None;
                }
                let text = std::fs::read_to_string(&p).ok()?;
                Some(json!({ "file": p.file_name().map(|n| n.to_string_lossy().into_owned()), "text": text }))
            });
            (!skills.is_empty() || !servers.is_empty() || instructions.is_some()).then(|| {
                json!({ "id": t.id, "name": t.name, "skills": skills, "mcp": servers, "instructions": instructions })
            })
        })
        .collect()
}

async fn scan() -> (StatusCode, Json<Value>) {
    let Some(home) = home() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "no home directory" })));
    };
    let tools = tokio::task::spawn_blocking(move || scan_home(&home)).await.unwrap_or_default();
    (StatusCode::OK, Json(json!({ "tools": tools })))
}

#[derive(Deserialize)]
struct SkillsBody {
    source: String,
    names: Vec<String>,
}

/// Copy one skill folder (regular files only, no symlinks) as `(relative path, bytes)`.
fn read_skill_folder(dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>, total: &mut usize) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if path.file_name().is_some_and(|n| n == ".git" || n == "node_modules") {
                    continue;
                }
                walk(base, &path, out, total)?;
            } else if meta.is_file() {
                *total += meta.len() as usize;
                if *total > 25 * 1024 * 1024 {
                    return Err("skill is larger than the install limit".into());
                }
                let rel = path.strip_prefix(base).map_err(|e| e.to_string())?.to_path_buf();
                out.push((rel, std::fs::read(&path).map_err(|e| e.to_string())?));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut total = 0usize;
    walk(dir, dir, &mut out, &mut total)?;
    Ok(out)
}

async fn import_skills(headers: HeaderMap, Json(body): Json<SkillsBody>) -> (StatusCode, Json<Value>) {
    let (Some(home), Some(root)) = (home(), crate::skill_catalog_install::skills_root()) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "no home directory" })));
    };
    if !TOOLS.iter().any(|t| t.id == body.source) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "unknown source" })));
    }
    let source = body.source.clone();
    let names = body.names.clone();
    let results = tokio::task::spawn_blocking(move || {
        let found = find_skills(&skill_roots(&home, &source));
        let versions = crate::skill_catalog_install::versions_root();
        let _ = std::fs::create_dir_all(&root);
        names
            .iter()
            .map(|name| {
                let Some((_, dir)) = found.iter().find(|(n, _)| n == name) else {
                    return json!({ "name": name, "installed": false, "error": "not found" });
                };
                let folder: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect();
                let files = match read_skill_folder(dir) {
                    Ok(f) => f,
                    Err(e) => return json!({ "name": name, "installed": false, "error": e }),
                };
                let Some(versions) = versions.as_ref() else {
                    return json!({ "name": name, "installed": false, "error": "no home directory" });
                };
                let id = format!("import-{source}-{folder}");
                let info = json!({ "kind": "import", "from": source, "path": dir });
                match crate::skill_catalog_install::write_skill(&root, versions, &id, &folder, &files, info) {
                    Ok(outcome) => json!({ "name": folder, "installed": true, "unchanged": outcome.unchanged }),
                    Err((_, message)) => json!({ "name": name, "installed": false, "error": message }),
                }
            })
            .collect::<Vec<Value>>()
    })
    .await
    .unwrap_or_default();
    let changed = results.iter().any(|r| r["installed"] == json!(true) && r["unchanged"] != json!(true));
    let reloaded = changed && crate::skill_catalog_install::reload_gizzi_skills(&headers).await;
    (StatusCode::OK, Json(json!({ "results": results, "agentReloaded": reloaded })))
}

pub fn agent_import_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent-import/scan", get(scan))
        .route("/agent-import/skills", post(import_skills))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_servers_reads_url_and_command_and_only_secret_names() {
        let text = r#"
model = "gpt-5"
[mcp_servers.linear]
url = "https://mcp.linear.app/mcp"
[mcp_servers.linear.http_headers]
Authorization = "Bearer sk-secret"
[mcp_servers."local-fs"]
command = "/usr/local/bin/fs-server"
env = { ROOT = "/tmp", TOKEN = "abc" }
"#;
        let servers = codex_servers(text);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0]["name"], "linear");
        assert_eq!(servers[0]["importable"], true);
        assert_eq!(servers[0]["secrets"], json!(["Authorization"]));
        assert_eq!(servers[1]["name"], "local-fs");
        assert_eq!(servers[1]["importable"], false);
        assert_eq!(servers[1]["command"], "fs-server");
        assert_eq!(servers[1]["secrets"], json!(["ROOT", "TOKEN"]));
        assert!(!servers.iter().any(|s| s.to_string().contains("sk-secret") || s.to_string().contains("abc")));
    }

    #[test]
    fn scan_finds_skills_servers_and_instructions_without_secret_values() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude/skills/pdf")).unwrap();
        std::fs::write(h.join(".claude/skills/pdf/SKILL.md"), "---\nname: pdf\n---\n").unwrap();
        std::fs::write(h.join(".claude/CLAUDE.md"), "Be brief.").unwrap();
        std::fs::write(
            h.join(".claude.json"),
            r#"{"mcpServers":{"notion":{"type":"http","url":"https://mcp.notion.com/mcp","headers":{"Authorization":"Bearer zzz"}}}}"#,
        )
        .unwrap();
        let tools = scan_home(h);
        let cc = tools.iter().find(|t| t["id"] == "claude-code").expect("claude code found");
        assert_eq!(cc["skills"][0]["name"], "pdf");
        assert_eq!(cc["mcp"][0]["url"], "https://mcp.notion.com/mcp");
        assert_eq!(cc["instructions"]["text"], "Be brief.");
        assert!(!cc.to_string().contains("zzz"));
    }
}
