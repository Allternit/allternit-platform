//! One-click local connector: write Allternit's local MCP server into a vendor app's own
//! MCP config on this computer, so a vendor bot's ticket can be run by that app on the
//! person's own subscription (`claude -p`, `codex exec`, ...).
//!
//! * The entry points at `http://127.0.0.1:<port>/mcp/bots/<vendorBotId>` with a **scoped
//!   key**: an `access_tokens` row whose only scope is `vendor-bot:<vendorBotId>`, good for that
//!   one connector path and nothing else (see `enterprise_auth::CredentialContext`).
//! * We only ever touch the entry we own (`allternit-<bot>`). Every other entry is carried over
//!   untouched, and the file is copied to `<file>.allternit-backup-<unix time>` before any write.
//!   A file we cannot parse is left alone and the connect fails.
//! * Undo removes our entry (and revokes the key). The backup stays next to the file.
//! * Vendor formats, checked against the vendors' docs on 2026-10-03:
//!   Claude Code `claude mcp add --transport http --scope user <name> <url> --header ...`
//!   (<https://code.claude.com/docs/en/mcp>); Codex `~/.codex/config.toml`
//!   `[mcp_servers.<name>]` `url` + `http_headers` (<https://learn.chatgpt.com/docs/extend/mcp?surface=cli>);
//!   Gemini CLI `~/.gemini/settings.json` `mcpServers.<name>.httpUrl` + `headers`
//!   (<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>);
//!   Claude Desktop `claude_desktop_config.json` (stdio entries only, so the bridge is
//!   `npx mcp-remote`) (<https://modelcontextprotocol.io/quickstart/user>);
//!   Hermes `~/.hermes/config.yaml` `mcp_servers.<name>.url` + `headers`
//!   (<https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp>).
//! * Headless approval (least privilege, scoped to OUR server entry only, never a global
//!   "approve everything"): Codex `default_tools_approval_mode = "approve"` in
//!   `[mcp_servers.<name>]` (same Codex MCP doc); Gemini CLI `"trust": true` on our
//!   `mcpServers.<name>` entry (bypasses confirmations for that server only, Gemini MCP doc
//!   above); Claude Code `--allowedTools mcp__<name>`. Hermes documents no MCP approval gate
//!   (tools are filtered by `tools.include/exclude`, never prompted), so it needs nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Extension, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

pub const APPS: [&str; 5] = ["claude_desktop", "claude_code", "codex", "gemini_cli", "hermes"];

pub fn is_app(app: &str) -> bool {
    APPS.contains(&app)
}

/// Claude Desktop has no headless mode: it can be connected, but a ticket cannot be run in it.
pub fn runnable(app: &str) -> bool {
    app != "claude_desktop"
}

pub fn app_label(app: &str) -> &'static str {
    match app {
        "claude_desktop" => "Claude Desktop",
        "claude_code" => "Claude Code",
        "codex" => "Codex",
        "gemini_cli" => "Gemini CLI",
        _ => "Hermes",
    }
}

/// The local apps a vendor's account can use, best first.
pub fn vendor_apps(vendor: &str) -> &'static [&'static str] {
    match vendor {
        "anthropic" => &["claude_code", "claude_desktop"],
        "openai" => &["codex"],
        "google" => &["gemini_cli"],
        "hermes" => &["hermes"],
        _ => &[],
    }
}

fn binary(app: &str) -> &'static str {
    match app {
        "claude_desktop" | "claude_code" => "claude",
        "codex" => "codex",
        "gemini_cli" => "gemini",
        _ => "hermes",
    }
}

/// The server name we own inside the app's config.
pub fn server_name(vendor_bot_id: &str) -> String {
    let clean: String = vendor_bot_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect::<String>().to_lowercase();
    format!("allternit-{clean}")
}

/// The vendor bot an entry's scoped key is limited to.
pub fn key_scope(vendor_bot_id: &str) -> String {
    format!("vendor-bot:{vendor_bot_id}")
}

pub fn local_mcp_url(vendor_bot_id: &str) -> String {
    let base = std::env::var("ALLTERNIT_LOCAL_MCP_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| {
        let port = std::env::var("ALLTERNIT_API_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8013);
        format!("http://127.0.0.1:{port}")
    });
    format!("{}/mcp/bots/{vendor_bot_id}", base.trim_end_matches('/'))
}

/// The command a headless run of `app` uses for one ticket nudge.
pub fn headless_command(app: &str, server_name: &str, prompt: &str) -> Option<(String, Vec<String>)> {
    let s = |x: &str| x.to_string();
    Some(match app {
        "claude_code" => (s("claude"), vec![s("-p"), prompt.into(), s("--allowedTools"), format!("mcp__{server_name}")]),
        "codex" => (s("codex"), vec![s("exec"), prompt.into()]),
        "gemini_cli" => (s("gemini"), vec![s("-p"), prompt.into()]),
        "hermes" => (s("hermes"), vec![s("chat"), s("-q"), prompt.into()]),
        _ => return None,
    })
}

// ─── Host seams ────────────────────────────────────────────────────────────────

pub struct CmdOut {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait CmdRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[String], timeout_secs: u64) -> Result<CmdOut, String>;
    fn which(&self, program: &str) -> bool;
}

pub struct SystemRunner;

#[async_trait]
impl CmdRunner for SystemRunner {
    async fn run(&self, program: &str, args: &[String], timeout_secs: u64) -> Result<CmdOut, String> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output())
            .await
            .map_err(|_| format!("{program} didn't finish in time"))?
            .map_err(|e| format!("{program} couldn't be started: {e}"))?;
        Ok(CmdOut { ok: out.status.success(), stdout: String::from_utf8_lossy(&out.stdout).into(), stderr: String::from_utf8_lossy(&out.stderr).into() })
    }
    fn which(&self, program: &str) -> bool {
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file())).unwrap_or(false)
    }
}

/// Checks that the local MCP server answers on an entry's URL with its key.
#[async_trait]
pub trait Prober: Send + Sync {
    async fn probe(&self, url: &str, key: &str) -> Result<(), String>;
}

pub struct HttpProber;

#[async_trait]
impl Prober for HttpProber {
    async fn probe(&self, url: &str, key: &str) -> Result<(), String> {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build().map_err(|e| e.to_string())?;
        let r = client
            .post(url)
            .bearer_auth(key)
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "allternit-health", "version": "1" } } }))
            .send()
            .await
            .map_err(|_| "Allternit's local server didn't answer. Is Allternit running?".to_string())?;
        if !r.status().is_success() {
            return Err(format!("Allternit's local server refused the key ({}).", r.status().as_u16()));
        }
        let v: Value = r.json().await.map_err(|_| "Allternit's local server answered something unreadable.".to_string())?;
        v["result"]["serverInfo"]["name"].as_str().map(|_| ()).ok_or_else(|| "That address isn't Allternit's connector.".to_string())
    }
}

pub struct Host<'a> {
    pub home: &'a Path,
    pub runner: &'a dyn CmdRunner,
    pub prober: &'a dyn Prober,
    pub unix_now: i64,
}

pub struct Entry<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub key: &'a str,
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// The config file Allternit edits (Claude Code's `~/.claude.json` is only backed up; the
/// entry itself goes in through `claude mcp add`).
pub fn config_path(app: &str, home: &Path) -> PathBuf {
    match app {
        "claude_desktop" => {
            if cfg!(target_os = "macos") {
                home.join("Library/Application Support/Claude/claude_desktop_config.json")
            } else if cfg!(windows) {
                std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Roaming")).join("Claude/claude_desktop_config.json")
            } else {
                home.join(".config/Claude/claude_desktop_config.json")
            }
        }
        "claude_code" => home.join(".claude.json"),
        "codex" => std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex")).join("config.toml"),
        "gemini_cli" => home.join(".gemini/settings.json"),
        _ => home.join(".hermes/config.yaml"),
    }
}

/// Is the app on this computer? (Config-file apps count when their config folder exists.)
pub fn installed(app: &str, home: &Path, runner: &dyn CmdRunner) -> bool {
    match app {
        "claude_desktop" => config_path(app, home).parent().is_some_and(Path::is_dir),
        _ => runner.which(binary(app)),
    }
}

// ─── Pure writers ──────────────────────────────────────────────────────────────

fn json_entry(app: &str, e: &Entry) -> Value {
    if app == "claude_desktop" {
        // Claude Desktop's file takes stdio servers only; mcp-remote bridges to our HTTP endpoint.
        json!({ "command": "npx", "args": ["-y", "mcp-remote", e.url, "--allow-http", "--header", "Authorization:${ALLTERNIT_AUTH}"], "env": { "ALLTERNIT_AUTH": format!("Bearer {}", e.key) } })
    } else {
        // `trust` skips tool-call confirmations for this server only, so a headless `gemini -p` can run it.
        json!({ "httpUrl": e.url, "headers": { "Authorization": format!("Bearer {}", e.key) }, "trust": true })
    }
}

fn parse_json_config(existing: &str) -> Result<Value, String> {
    if existing.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(existing).map_err(|_| "Its config file isn't valid JSON, so Allternit left it alone.".to_string())?;
    if v.is_object() { Ok(v) } else { Err("Its config file isn't a JSON object, so Allternit left it alone.".into()) }
}

pub fn write_json_entry(existing: &str, app: &str, e: &Entry) -> Result<String, String> {
    let mut v = parse_json_config(existing)?;
    let servers = v.as_object_mut().unwrap().entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else { return Err("Its config has an mcpServers that isn't an object, so Allternit left it alone.".into()) };
    servers.insert(e.name.to_string(), json_entry(app, e));
    Ok(serde_json::to_string_pretty(&v).unwrap() + "\n")
}

pub fn remove_json_entry(existing: &str, name: &str) -> Result<String, String> {
    let mut v = parse_json_config(existing)?;
    if let Some(s) = v.get_mut("mcpServers").and_then(Value::as_object_mut) {
        s.remove(name);
    }
    Ok(serde_json::to_string_pretty(&v).unwrap() + "\n")
}

fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// `[mcp_servers.<name>]` and `[mcp_servers.<name>.*]` blocks, header to the next `[` line.
fn strip_toml_tables(existing: &str, name: &str) -> String {
    let head = format!("[mcp_servers.{name}");
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in existing.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            skipping = t.starts_with(&head) && t[head.len()..].starts_with([']', '.']);
        }
        if !skipping {
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

pub fn write_toml_entry(existing: &str, e: &Entry) -> String {
    let mut base = strip_toml_tables(existing, e.name);
    if !base.is_empty() {
        base.push_str("\n\n");
    }
    // `approve` for this server's tools only, so a headless `codex exec` doesn't stall on a prompt.
    format!("{base}[mcp_servers.{}]\nurl = {}\nhttp_headers = {{ Authorization = {} }}\ndefault_tools_approval_mode = \"approve\"\n", e.name, toml_str(e.url), toml_str(&format!("Bearer {}", e.key)))
}

pub fn remove_toml_entry(existing: &str, name: &str) -> String {
    let s = strip_toml_tables(existing, name);
    if s.is_empty() { s } else { s + "\n" }
}

fn parse_yaml_config(existing: &str) -> Result<serde_yaml::Mapping, String> {
    if existing.trim().is_empty() {
        return Ok(Default::default());
    }
    match serde_yaml::from_str::<serde_yaml::Value>(existing) {
        Ok(serde_yaml::Value::Mapping(m)) => Ok(m),
        Ok(serde_yaml::Value::Null) => Ok(Default::default()),
        _ => Err("Its config file isn't valid YAML, so Allternit left it alone.".into()),
    }
}

pub fn write_yaml_entry(existing: &str, e: &Entry) -> Result<String, String> {
    let mut root = parse_yaml_config(existing)?;
    let key = serde_yaml::Value::from("mcp_servers");
    let mut servers = match root.remove(&key) {
        Some(serde_yaml::Value::Mapping(m)) => m,
        Some(serde_yaml::Value::Null) | None => Default::default(),
        Some(_) => return Err("Its config has an mcp_servers that isn't a map, so Allternit left it alone.".into()),
    };
    let entry: serde_yaml::Value = serde_yaml::to_value(json!({ "url": e.url, "headers": { "Authorization": format!("Bearer {}", e.key) } })).unwrap();
    servers.insert(e.name.into(), entry);
    root.insert(key, servers.into());
    serde_yaml::to_string(&root).map_err(|e| e.to_string())
}

pub fn remove_yaml_entry(existing: &str, name: &str) -> Result<String, String> {
    let mut root = parse_yaml_config(existing)?;
    if let Some(serde_yaml::Value::Mapping(s)) = root.get_mut("mcp_servers") {
        s.remove(name);
    }
    serde_yaml::to_string(&root).map_err(|e| e.to_string())
}

fn apply(app: &str, existing: &str, e: &Entry) -> Result<String, String> {
    match app {
        "codex" => Ok(write_toml_entry(existing, e)),
        "hermes" => write_yaml_entry(existing, e),
        _ => write_json_entry(existing, app, e),
    }
}

fn unapply(app: &str, existing: &str, name: &str) -> Result<String, String> {
    match app {
        "codex" => Ok(remove_toml_entry(existing, name)),
        "hermes" => remove_yaml_entry(existing, name),
        _ => remove_json_entry(existing, name),
    }
}

// ─── Host operations ───────────────────────────────────────────────────────────

fn io(e: std::io::Error) -> String {
    format!("Couldn't write the app's config: {e}")
}

/// Copy `path` next to itself before it changes. `None` when there is no file yet.
fn backup(path: &Path, unix_now: i64) -> Result<Option<PathBuf>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".allternit-backup-{unix_now}"));
    let dest = path.with_file_name(name);
    std::fs::copy(path, &dest).map_err(io)?;
    Ok(Some(dest))
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("allternit-tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(io)
}

#[derive(Debug, PartialEq)]
pub struct Written {
    pub config_path: PathBuf,
    pub backup_path: Option<PathBuf>,
}

pub async fn connect(h: &Host<'_>, app: &str, e: &Entry<'_>) -> Result<Written, String> {
    let path = config_path(app, h.home);
    if app == "claude_code" {
        if !h.runner.which("claude") {
            return Err("Claude Code isn't installed on this computer.".into());
        }
        let backup_path = backup(&path, h.unix_now)?;
        // Re-connecting replaces our own entry; a missing one is not an error.
        let _ = h.runner.run("claude", &["mcp".into(), "remove".into(), "--scope".into(), "user".into(), e.name.into()], 30).await;
        let args = ["mcp", "add", "--transport", "http", "--scope", "user", e.name, e.url, "--header", &format!("Authorization: Bearer {}", e.key)].map(String::from);
        let out = h.runner.run("claude", &args, 30).await?;
        if !out.ok {
            return Err(format!("Claude Code didn't accept the connector: {}", out.stderr.trim().chars().take(200).collect::<String>()));
        }
        return Ok(Written { config_path: path, backup_path });
    }
    let existing = if path.is_file() { std::fs::read_to_string(&path).map_err(io)? } else { String::new() };
    let updated = apply(app, &existing, e)?; // refuses before anything is written
    let backup_path = backup(&path, h.unix_now)?;
    write_private(&path, &updated)?;
    Ok(Written { config_path: path, backup_path })
}

pub async fn disconnect(h: &Host<'_>, app: &str, name: &str) -> Result<(), String> {
    let path = config_path(app, h.home);
    if app == "claude_code" {
        if !h.runner.which("claude") {
            return Ok(());
        }
        let out = h.runner.run("claude", &["mcp".into(), "remove".into(), "--scope".into(), "user".into(), name.into()], 30).await?;
        // "No MCP server found" means it is already gone.
        return if out.ok || out.stderr.to_lowercase().contains("no mcp server") { Ok(()) } else { Err(format!("Claude Code didn't remove it: {}", out.stderr.trim())) };
    }
    if !path.is_file() {
        return Ok(());
    }
    let existing = std::fs::read_to_string(&path).map_err(io)?;
    let updated = unapply(app, &existing, name)?;
    backup(&path, h.unix_now)?;
    write_private(&path, &updated)
}

/// The entry is in the app's config and Allternit's local server answers on it.
pub async fn health(h: &Host<'_>, app: &str, e: &Entry<'_>) -> Result<(), String> {
    if app == "claude_code" {
        let out = h.runner.run("claude", &["mcp".into(), "get".into(), e.name.into()], 30).await?;
        if !out.ok {
            return Err("Claude Code doesn't list the Allternit connector any more.".into());
        }
    } else {
        let path = config_path(app, h.home);
        let text = std::fs::read_to_string(&path).map_err(|_| format!("{}'s config file is gone.", app_label(app)))?;
        if !text.contains(e.name) || !text.contains(e.url) {
            return Err(format!("{} doesn't list the Allternit connector any more.", app_label(app)));
        }
    }
    h.prober.probe(e.url, e.key).await
}

// ─── Scoped key ────────────────────────────────────────────────────────────────

/// A key good for one vendor bot's connector path only. Returns `(id, plaintext)`.
pub fn mint_key(db: &DbHandle, user: &AuthUser, vendor_bot_id: &str) -> Result<(String, String), String> {
    crate::enterprise_auth::mint_scoped_key(db, user, &format!("Allternit local connector ({vendor_bot_id})"), &[key_scope(vendor_bot_id)], 365)
}

pub fn revoke_key(db: &DbHandle, owner: &str, key_id: &str) {
    if let Ok(c) = db.connect() {
        let _ = c.execute("UPDATE access_tokens SET revoked = 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2", params![key_id, owner]);
    }
}

// ─── Service ───────────────────────────────────────────────────────────────────

pub struct Row {
    pub app: String,
    pub vendor_bot_id: String,
    pub server_name: String,
    pub state: String,
    pub backup_path: Option<String>,
    pub key_id: Option<String>,
    pub error: Option<String>,
    pub connected_at: String,
    pub checked_at: Option<String>,
}

fn load_rows(db: &DbHandle, owner: &str, vendor_bot_id: Option<&str>) -> Vec<Row> {
    let Ok(c) = db.connect() else { return vec![] };
    let Ok(mut q) = c.prepare(
        "SELECT app, vendor_bot_id, server_name, state, backup_path, key_id, error, connected_at, checked_at FROM vendor_local_connectors
         WHERE owner = ?1 AND (?2 IS NULL OR vendor_bot_id = ?2) AND state <> 'removed' ORDER BY connected_at",
    ) else {
        return vec![];
    };
    q.query_map(params![owner, vendor_bot_id], |r| {
        Ok(Row { app: r.get(0)?, vendor_bot_id: r.get(1)?, server_name: r.get(2)?, state: r.get(3)?, backup_path: r.get(4)?, key_id: r.get(5)?, error: r.get(6)?, connected_at: r.get(7)?, checked_at: r.get(8)? })
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Apps that are connected and healthy for `owner`, for lane ranking.
pub fn connected_apps(db: &DbHandle, owner: &str) -> Vec<String> {
    load_rows(db, owner, None).into_iter().filter(|r| r.state == "connected").map(|r| r.app).collect()
}

/// The connected row for `app` on one vendor bot.
pub fn connected_server(db: &DbHandle, owner: &str, app: &str, vendor_bot_id: &str) -> Option<String> {
    load_rows(db, owner, Some(vendor_bot_id)).into_iter().find(|r| r.app == app && r.state == "connected").map(|r| r.server_name)
}

fn row_json(r: &Row) -> Value {
    json!({ "app": r.app, "label": app_label(&r.app), "vendorBotId": r.vendor_bot_id, "serverName": r.server_name, "state": r.state, "backupPath": r.backup_path, "error": r.error, "connectedAt": r.connected_at, "checkedAt": r.checked_at })
}

fn set_row(db: &DbHandle, owner: &str, app: &str, bot: &str, state: &str, error: Option<&str>) {
    if let Ok(c) = db.connect() {
        let _ = c.execute(
            "UPDATE vendor_local_connectors SET state = ?4, error = ?5, checked_at = ?6 WHERE owner = ?1 AND app = ?2 AND vendor_bot_id = ?3",
            params![owner, app, bot, state, error, chrono::Utc::now().to_rfc3339()],
        );
    }
}

/// Connect `app` to one vendor bot: key, config, health check. A failed check undoes the write.
pub async fn connect_app(db: &DbHandle, h: &Host<'_>, user: &AuthUser, app: &str, vendor_bot_id: &str) -> Result<Value, String> {
    if !is_app(app) {
        return Err("That isn't an app Allternit can connect to.".into());
    }
    if !installed(app, h.home, h.runner) {
        return Err(format!("{} isn't installed on this computer.", app_label(app)));
    }
    let name = server_name(vendor_bot_id);
    let url = local_mcp_url(vendor_bot_id);
    // Reconnecting rotates the key.
    let old_key = load_rows(db, &user.user_id, Some(vendor_bot_id)).into_iter().find(|r| r.app == app).and_then(|r| r.key_id);
    let (key_id, key) = mint_key(db, user, vendor_bot_id)?;
    let entry = Entry { name: &name, url: &url, key: &key };
    let written = match connect(h, app, &entry).await {
        Ok(w) => w,
        Err(e) => {
            revoke_key(db, &user.user_id, &key_id);
            return Err(e);
        }
    };
    if let Err(e) = health(h, app, &entry).await {
        let _ = disconnect(h, app, &name).await;
        revoke_key(db, &user.user_id, &key_id);
        return Err(e);
    }
    if let Some(k) = old_key {
        revoke_key(db, &user.user_id, &k);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let c = db.connect().map_err(|e| e.to_string())?;
    c.execute(
        "INSERT INTO vendor_local_connectors (owner, app, vendor_bot_id, server_name, state, config_path, backup_path, key_id, error, connected_at, checked_at)
         VALUES (?1,?2,?3,?4,'connected',?5,?6,?7,NULL,?8,?8)
         ON CONFLICT(owner, app, vendor_bot_id) DO UPDATE SET state='connected', config_path=excluded.config_path, backup_path=excluded.backup_path,
            key_id=excluded.key_id, error=NULL, connected_at=excluded.connected_at, checked_at=excluded.checked_at",
        params![user.user_id, app, vendor_bot_id, name, written.config_path.to_string_lossy(), written.backup_path.as_ref().map(|p| p.to_string_lossy().to_string()), key_id, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(json!({ "app": app, "label": app_label(app), "vendorBotId": vendor_bot_id, "serverName": name, "state": "connected", "configPath": written.config_path, "backupPath": written.backup_path }))
}

/// Re-check a connected app. The scoped key is not kept in plaintext, so a check only probes
/// with a fresh key minted and revoked within the call.
pub async fn check_app(db: &DbHandle, h: &Host<'_>, user: &AuthUser, app: &str, vendor_bot_id: &str) -> Result<Value, String> {
    let row = load_rows(db, &user.user_id, Some(vendor_bot_id)).into_iter().find(|r| r.app == app).ok_or("That app isn't connected.")?;
    let url = local_mcp_url(vendor_bot_id);
    let (probe_id, probe_key) = mint_key(db, user, vendor_bot_id)?;
    let res = health(h, app, &Entry { name: &row.server_name, url: &url, key: &probe_key }).await;
    revoke_key(db, &user.user_id, &probe_id);
    match &res {
        Ok(()) => set_row(db, &user.user_id, app, vendor_bot_id, "connected", None),
        Err(e) => set_row(db, &user.user_id, app, vendor_bot_id, "unhealthy", Some(e)),
    }
    let row = load_rows(db, &user.user_id, Some(vendor_bot_id)).into_iter().find(|r| r.app == app).ok_or("That app isn't connected.")?;
    Ok(json!({ "ok": res.is_ok(), "error": res.err(), "connector": row_json(&row) }))
}

pub async fn disconnect_app(db: &DbHandle, h: &Host<'_>, owner: &str, app: &str, vendor_bot_id: &str) -> Result<(), String> {
    let row = load_rows(db, owner, Some(vendor_bot_id)).into_iter().find(|r| r.app == app).ok_or("That app isn't connected.")?;
    disconnect(h, app, &row.server_name).await?;
    if let Some(k) = &row.key_id {
        revoke_key(db, owner, k);
    }
    set_row(db, owner, app, vendor_bot_id, "removed", None);
    Ok(())
}

// ─── HTTP ──────────────────────────────────────────────────────────────────────

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/vendor-local-connectors", get(list_h))
        .route("/v1/vendor-bots/:id/local-connectors/:app", post(connect_h).delete(disconnect_h))
        .route("/v1/vendor-bots/:id/local-connectors/:app/check", post(check_h))
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn owns_vendor_bot(state: &AppState, owner: &str, id: &str) -> bool {
    crate::mcp_vendor_bots::load_session(&state.db, owner, id).is_some()
}

async fn list_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let home = home_dir();
    let apps: Vec<Value> = APPS
        .iter()
        .map(|a| json!({ "app": a, "label": app_label(a), "installed": installed(a, &home, &SystemRunner), "runnable": runnable(a) }))
        .collect();
    let rows: Vec<Value> = load_rows(&state.db, &user.user_id, None).iter().map(row_json).collect();
    Json(json!({ "apps": apps, "connectors": rows })).into_response()
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn connect_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, UrlPath((id, app)): UrlPath<(String, String)>) -> Response {
    if !owns_vendor_bot(&state, &user.user_id, &id) {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    let home = home_dir();
    let h = Host { home: &home, runner: &SystemRunner, prober: &HttpProber, unix_now: unix_now() };
    match connect_app(&state.db, &h, &user, &app, &id).await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

async fn check_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, UrlPath((id, app)): UrlPath<(String, String)>) -> Response {
    if !owns_vendor_bot(&state, &user.user_id, &id) {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    let home = home_dir();
    let h = Host { home: &home, runner: &SystemRunner, prober: &HttpProber, unix_now: unix_now() };
    match check_app(&state.db, &h, &user, &app, &id).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

async fn disconnect_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, UrlPath((id, app)): UrlPath<(String, String)>) -> Response {
    if !owns_vendor_bot(&state, &user.user_id, &id) {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    let home = home_dir();
    let h = Host { home: &home, runner: &SystemRunner, prober: &HttpProber, unix_now: unix_now() };
    match disconnect_app(&state.db, &h, &user.user_id, &app, &id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        have: Vec<&'static str>,
        ran: Mutex<Vec<Vec<String>>>,
        fail_add: bool,
    }
    #[async_trait]
    impl CmdRunner for FakeRunner {
        async fn run(&self, program: &str, args: &[String], _t: u64) -> Result<CmdOut, String> {
            let mut line = vec![program.to_string()];
            line.extend(args.iter().cloned());
            self.ran.lock().unwrap().push(line);
            let ok = !(self.fail_add && args.get(1).map(String::as_str) == Some("add"));
            Ok(CmdOut { ok, stdout: String::new(), stderr: if ok { String::new() } else { "bad".into() } })
        }
        fn which(&self, p: &str) -> bool {
            self.have.contains(&p)
        }
    }
    struct Probe(Result<(), String>);
    #[async_trait]
    impl Prober for Probe {
        async fn probe(&self, _u: &str, _k: &str) -> Result<(), String> {
            self.0.clone()
        }
    }

    fn entry<'a>() -> Entry<'a> {
        Entry { name: "allternit-vb", url: "http://127.0.0.1:8013/mcp/bots/vb", key: "allternit_access_KEY" }
    }

    #[test]
    fn json_writers_keep_every_other_entry() {
        let existing = r#"{ "theme": "dark", "mcpServers": { "fs": { "command": "npx", "args": ["x"] } } }"#;
        for app in ["claude_desktop", "gemini_cli"] {
            let out: Value = serde_json::from_str(&write_json_entry(existing, app, &entry()).unwrap()).unwrap();
            assert_eq!(out["theme"], "dark");
            assert_eq!(out["mcpServers"]["fs"]["command"], "npx");
            assert!(out["mcpServers"]["allternit-vb"].is_object());
            // idempotent: writing twice leaves one entry
            let twice: Value = serde_json::from_str(&write_json_entry(&out.to_string(), app, &entry()).unwrap()).unwrap();
            assert_eq!(twice, out);
            let back: Value = serde_json::from_str(&remove_json_entry(&out.to_string(), "allternit-vb").unwrap()).unwrap();
            assert_eq!(back, serde_json::from_str::<Value>(existing).unwrap());
        }
        let g: Value = serde_json::from_str(&write_json_entry("", "gemini_cli", &entry()).unwrap()).unwrap();
        assert_eq!(g["mcpServers"]["allternit-vb"]["httpUrl"], "http://127.0.0.1:8013/mcp/bots/vb");
        assert_eq!(g["mcpServers"]["allternit-vb"]["headers"]["Authorization"], "Bearer allternit_access_KEY");
        let d: Value = serde_json::from_str(&write_json_entry("{}", "claude_desktop", &entry()).unwrap()).unwrap();
        assert_eq!(d["mcpServers"]["allternit-vb"]["command"], "npx");
        assert!(d["mcpServers"]["allternit-vb"]["args"].as_array().unwrap().iter().any(|a| a == "mcp-remote"));
        // unreadable files are refused, not overwritten
        assert!(write_json_entry("{ not json", "gemini_cli", &entry()).is_err());
        assert!(write_json_entry("[1]", "gemini_cli", &entry()).is_err());
        assert!(write_json_entry(r#"{"mcpServers": 3}"#, "gemini_cli", &entry()).is_err());
    }

    #[test]
    fn toml_writer_replaces_only_its_own_table() {
        let existing = "model = \"gpt-5\"\n\n# keep me\n[mcp_servers.docs]\nurl = \"https://docs.test/mcp\"\n\n[mcp_servers.allternit-vb]\nurl = \"old\"\n\n[mcp_servers.allternit-vb.extra]\nx = 1\n\n[profiles.fast]\nmodel = \"mini\"\n";
        let out = write_toml_entry(existing, &entry());
        assert!(out.contains("model = \"gpt-5\"") && out.contains("# keep me") && out.contains("[mcp_servers.docs]") && out.contains("[profiles.fast]"));
        assert_eq!(out.matches("[mcp_servers.allternit-vb]").count(), 1);
        assert!(!out.contains("url = \"old\"") && !out.contains("extra"));
        assert!(out.contains("url = \"http://127.0.0.1:8013/mcp/bots/vb\"") && out.contains("http_headers = { Authorization = \"Bearer allternit_access_KEY\" }"));
        let back = remove_toml_entry(&out, "allternit-vb");
        assert!(!back.contains("allternit-vb") && back.contains("[mcp_servers.docs]") && back.contains("[profiles.fast]"));
        assert_eq!(write_toml_entry("", &entry()).matches("[mcp_servers.").count(), 1);
        // a table that merely starts with the same letters is not ours
        let other = "[mcp_servers.allternit-vb2]\nurl = \"keep\"\n";
        assert!(remove_toml_entry(other, "allternit-vb").contains("allternit-vb2"));
    }

    #[test]
    fn headless_approval_is_scoped_to_our_server_entry_only() {
        // Codex: approve mode inside our own table, and nowhere global.
        let existing = "approval_policy = \"on-request\"\n\n[mcp_servers.docs]\nurl = \"https://docs.test/mcp\"\n";
        let out = write_toml_entry(existing, &entry());
        let (head, ours) = out.split_once("[mcp_servers.allternit-vb]").unwrap();
        assert!(ours.contains("default_tools_approval_mode = \"approve\""));
        assert!(!head.contains("default_tools_approval_mode"), "other tables and the root stay untouched");
        assert!(head.contains("approval_policy = \"on-request\""));
        assert_eq!(out.matches("default_tools_approval_mode").count(), 1);
        // Removing our entry removes the approval with it.
        assert!(!remove_toml_entry(&out, "allternit-vb").contains("default_tools_approval_mode"));
        // Gemini: `trust` on our entry only; other servers keep their own setting.
        let g: Value = serde_json::from_str(&write_json_entry(r#"{"mcpServers":{"fs":{"command":"x"}}}"#, "gemini_cli", &entry()).unwrap()).unwrap();
        assert_eq!(g["mcpServers"]["allternit-vb"]["trust"], true);
        assert!(g["mcpServers"]["fs"].get("trust").is_none());
        assert!(g.get("trust").is_none());
        // Claude Desktop's bridge entry carries no trust flag.
        let d: Value = serde_json::from_str(&write_json_entry("{}", "claude_desktop", &entry()).unwrap()).unwrap();
        assert!(d["mcpServers"]["allternit-vb"].get("trust").is_none());
    }

    #[test]
    fn yaml_writer_keeps_other_servers() {
        let existing = "model: x\nmcp_servers:\n  docs:\n    url: https://docs.test\n";
        let out: serde_yaml::Value = serde_yaml::from_str(&write_yaml_entry(existing, &entry()).unwrap()).unwrap();
        assert_eq!(out["model"], serde_yaml::Value::from("x"));
        assert_eq!(out["mcp_servers"]["docs"]["url"], serde_yaml::Value::from("https://docs.test"));
        assert_eq!(out["mcp_servers"]["allternit-vb"]["headers"]["Authorization"], serde_yaml::Value::from("Bearer allternit_access_KEY"));
        let back = remove_yaml_entry(&serde_yaml::to_string(&out).unwrap(), "allternit-vb").unwrap();
        assert!(!back.contains("allternit-vb") && back.contains("docs"));
        assert!(write_yaml_entry("a: [", &entry()).is_err());
        assert!(write_yaml_entry("mcp_servers: 3", &entry()).is_err());
    }

    #[tokio::test]
    async fn connect_backs_up_writes_and_undo_removes_only_ours() {
        let runner = FakeRunner::default();
        let probe = Probe(Ok(()));
        for (app, original) in [
            ("claude_desktop", r#"{"mcpServers":{"fs":{"command":"npx"}}}"#),
            ("gemini_cli", r#"{"mcpServers":{"fs":{"httpUrl":"https://x"}}}"#),
            ("codex", "[mcp_servers.docs]\nurl = \"https://docs.test\"\n"),
            ("hermes", "mcp_servers:\n  docs:\n    url: https://docs.test\n"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let h = Host { home: home.path(), runner: &runner, prober: &probe, unix_now: 1_700_000_000 };
            let path = config_path(app, home.path());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, original).unwrap();
            let w = connect(&h, app, &entry()).await.unwrap();
            let backup = w.backup_path.expect("an existing file is backed up first");
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), original, "{app}: backup is the untouched file");
            assert!(backup.to_string_lossy().ends_with(".allternit-backup-1700000000"));
            let written = std::fs::read_to_string(&path).unwrap();
            assert!(written.contains("allternit-vb") && written.contains("allternit_access_KEY"), "{app}");
            assert!(written.contains(if app == "codex" || app == "hermes" { "docs" } else { "fs" }), "{app}: the other entry survives");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "{app}: the key is in a private file");
            }
            health(&h, app, &entry()).await.unwrap();
            disconnect(&h, app, "allternit-vb").await.unwrap();
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(!after.contains("allternit-vb") && !after.contains("allternit_access_KEY"), "{app}");
            assert!(after.contains(if app == "codex" || app == "hermes" { "docs" } else { "fs" }), "{app}");
            assert!(health(&h, app, &entry()).await.is_err(), "{app}: gone is reported");
        }
    }

    #[tokio::test]
    async fn connect_creates_a_missing_config_and_leaves_an_unreadable_one_alone() {
        let runner = FakeRunner::default();
        let probe = Probe(Ok(()));
        let home = tempfile::tempdir().unwrap();
        let h = Host { home: home.path(), runner: &runner, prober: &probe, unix_now: 5 };
        let w = connect(&h, "gemini_cli", &entry()).await.unwrap();
        assert!(w.backup_path.is_none() && w.config_path.is_file());
        let path = config_path("hermes", home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "a: [").unwrap();
        assert!(connect(&h, "hermes", &entry()).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a: [", "an unparseable file is untouched");
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "and no backup litter");
    }

    #[tokio::test]
    async fn claude_code_goes_through_its_own_cli() {
        let runner = FakeRunner { have: vec!["claude"], ..Default::default() };
        let probe = Probe(Ok(()));
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".claude.json"), "{}").unwrap();
        let h = Host { home: home.path(), runner: &runner, prober: &probe, unix_now: 9 };
        let w = connect(&h, "claude_code", &entry()).await.unwrap();
        assert!(w.backup_path.unwrap().to_string_lossy().ends_with(".claude.json.allternit-backup-9"));
        let ran = runner.ran.lock().unwrap().clone();
        let add = ran.iter().find(|r| r[2] == "add").unwrap();
        assert_eq!(add[..8], ["claude", "mcp", "add", "--transport", "http", "--scope", "user", "allternit-vb"].map(String::from));
        assert_eq!(add[8..], ["http://127.0.0.1:8013/mcp/bots/vb", "--header", "Authorization: Bearer allternit_access_KEY"].map(String::from));
        health(&h, "claude_code", &entry()).await.unwrap();
        disconnect(&h, "claude_code", "allternit-vb").await.unwrap();
        assert!(runner.ran.lock().unwrap().iter().any(|r| r[1] == "mcp" && r[2] == "remove" && r.last().unwrap() == "allternit-vb"));
        // not installed
        let none = FakeRunner::default();
        let h2 = Host { home: home.path(), runner: &none, prober: &probe, unix_now: 9 };
        assert!(connect(&h2, "claude_code", &entry()).await.unwrap_err().contains("isn't installed"));
        // the CLI refusing is reported
        let bad = FakeRunner { have: vec!["claude"], fail_add: true, ..Default::default() };
        let h3 = Host { home: home.path(), runner: &bad, prober: &probe, unix_now: 9 };
        assert!(connect(&h3, "claude_code", &entry()).await.unwrap_err().contains("didn't accept"));
    }

    #[test]
    fn headless_commands_per_app() {
        let c = |a| headless_command(a, "allternit-vb", "Run Allternit ticket T-1.").unwrap();
        assert_eq!(c("claude_code"), ("claude".into(), ["-p", "Run Allternit ticket T-1.", "--allowedTools", "mcp__allternit-vb"].map(String::from).to_vec()));
        assert_eq!(c("codex"), ("codex".into(), ["exec", "Run Allternit ticket T-1."].map(String::from).to_vec()));
        assert_eq!(c("gemini_cli").1[0], "-p");
        assert_eq!(c("hermes").1[..2], ["chat", "-q"].map(String::from));
        assert!(headless_command("claude_desktop", "x", "p").is_none());
        assert_eq!(server_name("Bot_ID-123/../x"), "allternit-botid123x");
        assert_eq!(vendor_apps("anthropic")[0], "claude_code");
    }

    fn user() -> AuthUser {
        AuthUser { user_id: "user-a".into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    async fn state(tag: &str) -> Arc<AppState> {
        let st = crate::aai_facade::test_util::setup(tag, "READY").await;
        st.db.connect().unwrap().execute("INSERT OR IGNORE INTO users (id, email) VALUES ('user-a', 'a@test.dev')", []).unwrap();
        st
    }

    fn live_keys(st: &AppState) -> Vec<(String, i64)> {
        let c = st.db.connect().unwrap();
        let mut q = c.prepare("SELECT scopes, revoked FROM access_tokens ORDER BY created_at, id").unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    }

    #[tokio::test]
    async fn connect_app_mints_a_scoped_key_records_it_and_undo_revokes_it() {
        let st = state("lc-ok").await;
        let runner = FakeRunner { have: vec!["codex"], ..Default::default() };
        let probe = Probe(Ok(()));
        let home = tempfile::tempdir().unwrap();
        let h = Host { home: home.path(), runner: &runner, prober: &probe, unix_now: 1 };
        let v = connect_app(&st.db, &h, &user(), "codex", "bot-vendor").await.unwrap();
        assert_eq!((v["state"].as_str(), v["serverName"].as_str()), (Some("connected"), Some("allternit-botvendor")));
        let keys = live_keys(&st);
        assert_eq!(keys, [(r#"["vendor-bot:bot-vendor"]"#.to_string(), 0)], "one key, one scope");
        let toml = std::fs::read_to_string(config_path("codex", home.path())).unwrap();
        assert!(toml.contains("/mcp/bots/bot-vendor") && toml.contains("Bearer allternit_access_"));
        assert_eq!(connected_apps(&st.db, "user-a"), ["codex"]);
        assert_eq!(connected_server(&st.db, "user-a", "codex", "bot-vendor").as_deref(), Some("allternit-botvendor"));
        // reconnecting rotates the key
        connect_app(&st.db, &h, &user(), "codex", "bot-vendor").await.unwrap();
        assert_eq!(live_keys(&st).iter().filter(|k| k.1 == 0).count(), 1);
        assert_eq!(live_keys(&st).len(), 2);
        // check
        let c = check_app(&st.db, &h, &user(), "codex", "bot-vendor").await.unwrap();
        assert_eq!(c["ok"], true);
        assert_eq!(live_keys(&st).iter().filter(|k| k.1 == 0).count(), 1, "the check's probe key is revoked again");
        // undo
        disconnect_app(&st.db, &h, "user-a", "codex", "bot-vendor").await.unwrap();
        assert!(live_keys(&st).iter().all(|k| k.1 == 1));
        assert!(connected_apps(&st.db, "user-a").is_empty());
        assert!(!std::fs::read_to_string(config_path("codex", home.path())).unwrap().contains("allternit-botvendor"));
        assert!(disconnect_app(&st.db, &h, "user-a", "codex", "bot-vendor").await.is_err(), "already removed");
    }

    #[tokio::test]
    async fn a_failed_health_check_rolls_the_write_back() {
        let st = state("lc-bad").await;
        let runner = FakeRunner { have: vec!["gemini"], ..Default::default() };
        let probe = Probe(Err("Allternit's local server didn't answer.".into()));
        let home = tempfile::tempdir().unwrap();
        let path = config_path("gemini_cli", home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{"mcpServers":{"fs":{"command":"x"}}}"#;
        std::fs::write(&path, original).unwrap();
        let h = Host { home: home.path(), runner: &runner, prober: &probe, unix_now: 2 };
        let e = connect_app(&st.db, &h, &user(), "gemini_cli", "bot-vendor").await.unwrap_err();
        assert!(e.contains("didn't answer"));
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after, serde_json::from_str::<Value>(original).unwrap(), "our entry is gone, theirs untouched");
        assert!(live_keys(&st).iter().all(|k| k.1 == 1), "no live key is left behind");
        assert!(connected_apps(&st.db, "user-a").is_empty());
        // an app that isn't installed, and an unknown app
        assert!(connect_app(&st.db, &h, &user(), "codex", "bot-vendor").await.unwrap_err().contains("isn't installed"));
        assert!(connect_app(&st.db, &h, &user(), "notepad", "bot-vendor").await.is_err());
    }

    #[test]
    fn a_vendor_bot_key_opens_only_its_own_connector() {
        use crate::enterprise_auth::CredentialContext;
        use axum::http::Method;
        let ctx = CredentialContext { credential_id: "k".into(), kind: "access_token", scopes: vec![key_scope("vb")] };
        assert!(ctx.allows_request(&Method::POST, "/mcp/bots/vb"));
        assert!(ctx.allows_request(&Method::POST, "/api/v1/mcp/bots/vb"));
        assert!(!ctx.allows_request(&Method::POST, "/mcp/bots/other"), "another bot");
        assert!(!ctx.allows_request(&Method::POST, "/mcp/bots/vbx"));
        assert!(!ctx.allows_request(&Method::GET, "/mcp/bots/vb"));
        assert!(!ctx.allows_request(&Method::GET, "/api/v1/agents"), "nothing else");
        assert!(!ctx.allows_request(&Method::POST, "/api/v1/vault/credentials"));
        assert!(!ctx.allows_request(&Method::POST, "/mcp/server"));
    }
}
