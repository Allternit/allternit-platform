//! Live sessions (contract C6): every agent session running on this computer,
//! with a no-token `doing` line read from what the agent records anyway.
//!
//! Sources, merged by working directory: Claude Code's `~/.claude/sessions/<pid>.json`
//! (dead pids skipped), Codex's `$CODEX_HOME/session_index.jsonl`, Factory panes
//! (`agent_status` from the pane engine) and the shared hooks file WP11 writes at
//! `~/.allternit/live-sessions/hooks.json`. Status: Claude's own `status` wins, then
//! the pane's `agent_status`, then hooks.
//!
//! Portions adapted from Reason Network (reason-machines/reason-network), Apache-2.0. Copyright 2026 Reason Machines, Inc.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How much of a transcript is read the first time it is seen; after that, only what was appended.
const FIRST_READ: u64 = 2 << 20;
const MAX_DOING: usize = 120;
/// Codex has no pid: a thread counts as live while its index entry is this fresh.
const CODEX_LIVE_SECS: i64 = 15 * 60;
/// A hook seen this recently means the session is mid-turn.
const HOOK_BUSY_SECS: i64 = 30;
/// A hooks-only session (no pid, no pane) is listed while its last hook is this fresh.
const HOOK_LIVE_SECS: i64 = 10 * 60;

/// One live agent session (C6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSession {
    pub address: String,
    pub name: String,
    pub harness: String,
    pub machine: String,
    pub machine_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `busy | idle | offline`.
    pub status: String,
    pub since: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// `push | turn | pull` (C1).
    pub tier: String,
    pub unread: u32,
}

/// How WP11 (delivery) and WP13/14 look sessions up.
pub trait SessionLookup {
    fn by_address(address: &str) -> Option<LiveSession>;
    fn all() -> Vec<LiveSession>;
}

impl SessionLookup for LiveSession {
    fn by_address(address: &str) -> Option<LiveSession> {
        Self::all().into_iter().find(|s| s.address.eq_ignore_ascii_case(address))
    }
    fn all() -> Vec<LiveSession> {
        discover(&default_person(), None)
    }
}

/// `$ALLTERNIT_PERSON`, else the OS user. allternit-api passes the Clerk username explicitly.
pub fn default_person() -> String {
    std::env::var("ALLTERNIT_PERSON")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "me".into())
        .to_lowercase()
}

fn home() -> PathBuf {
    crate::platform::user_home_or("/")
}

fn claude_dir() -> PathBuf {
    home().join(".claude")
}

fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map_or_else(|| home().join(".codex"), PathBuf::from)
}

fn hooks_file() -> PathBuf {
    std::env::var_os("ALLTERNIT_HOOKS_FILE").map_or_else(|| home().join(".allternit").join("live-sessions").join("hooks.json"), PathBuf::from)
}

fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn rfc3339_ms(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_else(Utc::now).to_rfc3339()
}

fn machine() -> (String, String) {
    static M: OnceLock<(String, String)> = OnceLock::new();
    M.get_or_init(|| {
        let name = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().trim_end_matches(".local").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "this computer".into());
        let id = std::env::var("ALLTERNIT_RUNTIME_ID").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "local".into());
        (id, name)
    })
    .clone()
}

// ---------------------------------------------------------------- git

type GitInfo = (Option<String>, Option<String>);

/// `(owner/repo, branch)` for a directory, cached for 30 s (this is polled).
fn git_info(cwd: &str) -> GitInfo {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, GitInfo)>>> = OnceLock::new();
    if cwd.is_empty() {
        return (None, None);
    }
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = cache.get(cwd) {
        if at.elapsed() < Duration::from_secs(30) {
            return v.clone();
        }
    }
    let git = |args: &[&str]| -> Option<String> {
        let o = std::process::Command::new("git").arg("-C").arg(cwd).args(args).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
    };
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    let repo = git(&["remote", "get-url", "origin"]).and_then(|u| repo_slug(&u));
    cache.insert(cwd.to_string(), (Instant::now(), (repo.clone(), branch.clone())));
    (repo, branch)
}

/// `git@github.com:Allternit/x.git` / `https://github.com/Allternit/x` → `Allternit/x`.
fn repo_slug(url: &str) -> Option<String> {
    let t = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let mut parts = t.rsplit(|c| c == '/' || c == ':');
    let repo = parts.next()?;
    let owner = parts.next()?;
    (!repo.is_empty() && !owner.is_empty()).then(|| format!("{owner}/{repo}"))
}

// ---------------------------------------------------------------- doing

/// What a transcript says so far. `task` is `Some(None)` when its latest todo list has nothing in progress.
#[derive(Clone, Default)]
struct Seen {
    title: Option<String>,
    task: Option<Option<String>>,
}

impl Seen {
    fn visit(&mut self, v: &Value) {
        match v {
            Value::Object(o) => {
                if o.get("type").and_then(Value::as_str) == Some("ai-title") {
                    if let Some(t) = o.get("aiTitle").and_then(Value::as_str) {
                        self.title = Some(t.to_string());
                    }
                }
                for (k, v) in o {
                    match (k.as_str(), v) {
                        ("todos" | "plan", Value::Array(items)) if items.iter().any(|i| i.get("status").is_some()) => {
                            self.task = Some(items.iter().find(|i| in_progress(i)).and_then(task_text));
                        }
                        _ => self.visit(v),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| self.visit(i)),
            // Codex keeps tool arguments as a JSON string.
            Value::String(s) if s.starts_with('{') && s.contains("\"status\"") => {
                if let Ok(v) = serde_json::from_str::<Value>(s) {
                    self.visit(&v);
                }
            }
            _ => {}
        }
    }
}

fn in_progress(item: &Value) -> bool {
    matches!(item.get("status").and_then(Value::as_str), Some("in_progress" | "in-progress" | "active"))
}

fn task_text(item: &Value) -> Option<String> {
    ["activeForm", "content", "step", "description", "subject", "title"]
        .iter()
        .find_map(|k| item.get(*k).and_then(Value::as_str))
        .map(str::to_string)
}

struct Tail {
    modified: Option<SystemTime>,
    offset: u64,
    seen: Seen,
}

/// Reads what was appended to a transcript since last time (they only grow), or starts over if it shrank.
fn scan(path: &Path) -> Seen {
    static TAILS: OnceLock<Mutex<HashMap<PathBuf, Tail>>> = OnceLock::new();
    let Ok(meta) = std::fs::metadata(path) else { return Seen::default() };
    let (len, modified) = (meta.len(), meta.modified().ok());
    let mut tails = TAILS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let t = tails.entry(path.to_path_buf()).or_insert(Tail { modified: None, offset: 0, seen: Seen::default() });
    if t.modified == modified && t.offset == len {
        return t.seen.clone();
    }
    if len < t.offset {
        *t = Tail { modified: None, offset: 0, seen: Seen::default() };
    }
    let start = if t.offset == 0 { len.saturating_sub(FIRST_READ) } else { t.offset };
    let mut buf = Vec::new();
    let read = File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(start))?;
        f.take(len - start).read_to_end(&mut buf)
    });
    if read.is_err() {
        return t.seen.clone();
    }
    t.modified = modified;
    // Up to the last complete line; a line still being written is read next time.
    let end = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    // A first read that began mid-file starts mid-line: drop the partial first line.
    let skip = if start > 0 && t.offset == 0 { buf.iter().position(|&b| b == b'\n').map_or(end, |i| i + 1) } else { 0 };
    for line in buf[skip.min(end)..end].split(|&b| b == b'\n') {
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            t.seen.visit(&v);
        }
    }
    t.offset = start + end as u64;
    t.seen.clone()
}

fn claude_transcript(session: &str) -> Option<PathBuf> {
    static FOUND: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let mut found = FOUND.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = found.get(session).filter(|p| p.exists()) {
        return Some(p.clone());
    }
    let file = format!("{session}.jsonl");
    let path = std::fs::read_dir(claude_dir().join("projects")).ok()?.flatten().map(|e| e.path().join(&file)).find(|p| p.exists())?;
    found.insert(session.to_string(), path.clone());
    Some(path)
}

/// The in-progress task with the highest id on Claude's task list (`~/.claude/tasks/<session>/<n>.json`).
fn claude_task(session: &str) -> Option<String> {
    let dir = std::fs::read_dir(claude_dir().join("tasks").join(session)).ok()?;
    dir.flatten()
        .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok())
        .filter(in_progress)
        .max_by_key(|t| {
            t.get("id").and_then(|i| i.as_str().map(str::to_string).or_else(|| i.as_u64().map(|n| n.to_string()))).and_then(|i| i.parse::<u64>().ok())
        })
        .and_then(|t| ["activeForm", "subject"].iter().find_map(|k| t.get(*k).and_then(Value::as_str)).map(str::to_string))
}

/// Codex thread names and last-update times from `session_index.jsonl` (appended to; the last entry wins).
fn codex_index() -> HashMap<String, (String, Option<DateTime<Utc>>)> {
    let path = codex_dir().join("session_index.jsonl");
    let Ok(mut file) = File::open(&path) else { return HashMap::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(1 << 20)));
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            let at = v.get("updated_at").and_then(Value::as_str).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc));
            Some((v.get("id")?.as_str()?.to_string(), (v.get("thread_name").and_then(Value::as_str).unwrap_or_default().to_string(), at)))
        })
        .collect()
}

fn clip(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= MAX_DOING {
        s
    } else {
        format!("{}…", s.chars().take(MAX_DOING - 1).collect::<String>())
    }
}

/// `title · task in progress`, either alone, or nothing. `transcript` is the one hooks reported, if any.
pub fn doing(harness: &str, session: &str, transcript: Option<&Path>) -> Option<String> {
    let claude = harness == "claude";
    let path = transcript.map(Path::to_path_buf).or_else(|| claude.then(|| claude_transcript(session)).flatten());
    let seen = path.as_deref().map(scan).unwrap_or_default();
    let task = claude.then(|| claude_task(session)).flatten().or(seen.task.flatten());
    let title = seen
        .title
        .or_else(|| (harness == "codex").then(|| codex_index().remove(session).map(|(n, _)| n).filter(|n| !n.is_empty())).flatten());
    let line = match (title, task) {
        (Some(t), Some(k)) if !t.eq_ignore_ascii_case(&k) => format!("{t} · {k}"),
        (Some(t), _) => t,
        (None, Some(k)) => k,
        (None, None) => return None,
    };
    Some(clip(&line))
}

// ---------------------------------------------------------------- discovery

fn norm_status(s: &str) -> Option<&'static str> {
    match s {
        "busy" | "working" | "running" | "thinking" => Some("busy"),
        "idle" | "waiting" | "done" | "blocked" | "ready" => Some("idle"),
        _ => None,
    }
}

/// What a hook told us about a session (WP11 writes the file; fields are read tolerantly).
#[derive(Default, Clone)]
struct HookInfo {
    session: String,
    harness: String,
    cwd: Option<String>,
    transcript: Option<PathBuf>,
    pid: Option<u32>,
    last_hook: Option<DateTime<Utc>>,
}

fn read_hooks() -> Vec<HookInfo> {
    let Ok(bytes) = std::fs::read(hooks_file()) else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return Vec::new() };
    let root = v.get("sessions").unwrap_or(&v);
    let entries: Vec<(Option<String>, &Value)> = match root {
        Value::Object(m) => m.iter().map(|(k, v)| (Some(k.clone()), v)).collect(),
        Value::Array(a) => a.iter().map(|v| (None, v)).collect(),
        _ => Vec::new(),
    };
    let s = |v: &Value, ks: &[&str]| ks.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::to_string);
    entries
        .into_iter()
        .filter_map(|(key, v)| {
            let session = s(v, &["sessionId", "session_id", "session"]).or(key)?;
            let at = v.get("last_hook").or_else(|| v.get("lastHook")).and_then(|h| {
                h.get("at").unwrap_or(h).as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|d| d.with_timezone(&Utc))
            });
            Some(HookInfo {
                session,
                harness: s(v, &["harness", "agent"]).unwrap_or_else(|| "claude".into()),
                cwd: s(v, &["cwd"]),
                transcript: s(v, &["transcript", "transcriptPath", "transcript_path"]).map(PathBuf::from),
                pid: v.get("pid").and_then(Value::as_u64).map(|p| p as u32),
                last_hook: at,
            })
        })
        .collect()
}

fn pane_harness(session: &str) -> &'static str {
    let s = session.to_lowercase();
    ["claude", "codex", "gemini", "cursor", "copilot", "opencode", "pi"].into_iter().find(|h| s.contains(h)).unwrap_or("gizzi")
}

fn base_name(cwd: &str) -> String {
    Path::new(cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("session").to_string()
}

/// One session found by a source, before panes and addresses are applied.
struct Found {
    harness: String,
    id: String,
    pid: Option<u32>,
    cwd: String,
    own_status: Option<String>,
    since: String,
    socket: bool,
    hook: Option<HookInfo>,
}

/// Every live session on this computer. `person` is the address prefix (the Clerk username);
/// `panes` are the Factory panes (pass `None` to ask the installed pane engine).
pub fn discover(person: &str, panes: Option<Vec<super::backend::LivePane>>) -> Vec<LiveSession> {
    let share_paths = matches!(std::env::var("ALLTERNIT_SHARE_PATHS").as_deref(), Ok("1" | "true"));
    let (machine_id, machine_name) = machine();
    let panes = panes.unwrap_or_else(|| super::backend::backend().and_then(|b| b.list()).unwrap_or_default());
    let hooks = read_hooks();
    let now = Utc::now();
    let mut found: Vec<Found> = Vec::new();

    // Claude Code: one file per running process.
    if let Ok(dir) = std::fs::read_dir(claude_dir().join("sessions")) {
        let mut files: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for f in files {
            let Ok(v) = serde_json::from_slice::<Value>(&std::fs::read(&f).unwrap_or_default()) else { continue };
            let (Some(pid), Some(id)) = (v["pid"].as_u64().map(|p| p as u32), v["sessionId"].as_str()) else { continue };
            if !pid_alive(pid) {
                continue;
            }
            found.push(Found {
                harness: "claude".into(),
                id: id.to_string(),
                pid: Some(pid),
                cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
                own_status: v["status"].as_str().map(str::to_string),
                since: rfc3339_ms(v["startedAt"].as_i64().unwrap_or_else(|| now.timestamp_millis())),
                socket: v["messagingSocketPath"].as_str().is_some_and(|s| !s.is_empty()),
                hook: hooks.iter().find(|h| h.session == id).cloned(),
            });
        }
    }

    // Codex: no pid, so a thread is live while its index entry is fresh. Its cwd comes from hooks.
    for (id, (_, at)) in codex_index() {
        let Some(at) = at.filter(|t| (now - *t).num_seconds() <= CODEX_LIVE_SECS) else { continue };
        let hook = hooks.iter().find(|h| h.session == id).cloned();
        found.push(Found {
            harness: "codex".into(),
            pid: hook.as_ref().and_then(|h| h.pid),
            cwd: hook.as_ref().and_then(|h| h.cwd.clone()).unwrap_or_default(),
            id,
            own_status: None,
            since: at.to_rfc3339(),
            socket: false,
            hook,
        });
    }

    // Hooks-only sessions (other harnesses) while their last hook is fresh.
    for h in &hooks {
        let fresh = h.last_hook.is_some_and(|t| (now - t).num_seconds() <= HOOK_LIVE_SECS);
        if found.iter().any(|f| f.id == h.session) || !fresh || h.pid.is_some_and(|p| !pid_alive(p)) {
            continue;
        }
        found.push(Found {
            harness: h.harness.clone(),
            id: h.session.clone(),
            pid: h.pid,
            cwd: h.cwd.clone().unwrap_or_default(),
            own_status: None,
            since: h.last_hook.map(|t| t.to_rfc3339()).unwrap_or_default(),
            socket: false,
            hook: Some(h.clone()),
        });
    }

    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<LiveSession> = Vec::new();
    for f in found {
        let pane = panes.iter().find(|p| !used.contains(&p.pane_id) && !f.cwd.is_empty() && p.cwd.as_deref() == Some(f.cwd.as_str()));
        if let Some(p) = pane {
            used.insert(p.pane_id.clone());
        }
        let hook_busy = f.hook.as_ref().and_then(|h| h.last_hook).is_some_and(|t| (now - t).num_seconds() <= HOOK_BUSY_SECS);
        let status = f
            .own_status
            .as_deref()
            .and_then(norm_status)
            .or_else(|| pane.and_then(|p| p.agent_status.as_deref()).and_then(norm_status))
            .unwrap_or(if hook_busy { "busy" } else { "idle" });
        let tier = if f.socket || f.harness == "codex" || pane.is_some() {
            "push"
        } else if f.hook.is_some() {
            "turn"
        } else {
            "pull"
        };
        let (repo, branch) = git_info(&f.cwd);
        out.push(LiveSession {
            name: format!("{}-{}", base_name(&f.cwd), f.harness),
            doing: doing(&f.harness, &f.id, f.hook.as_ref().and_then(|h| h.transcript.as_deref())),
            harness: f.harness,
            session_id: f.id,
            pid: f.pid,
            pane_id: pane.map(|p| p.pane_id.clone()),
            cwd: if share_paths { f.cwd } else { base_name(&f.cwd) },
            repo,
            branch,
            status: status.into(),
            since: f.since,
            tier: tier.into(),
            ..Default::default()
        });
    }

    // Factory panes no session claimed (bots and plain panes).
    for p in panes.iter().filter(|p| !used.contains(&p.pane_id)) {
        let cwd = p.cwd.clone().unwrap_or_default();
        let harness = pane_harness(&p.session);
        let (repo, branch) = git_info(&cwd);
        out.push(LiveSession {
            name: format!("{}-{harness}", base_name(&cwd)),
            harness: harness.into(),
            session_id: p.session.clone(),
            pane_id: Some(p.pane_id.clone()),
            cwd: if share_paths { cwd } else { base_name(&cwd) },
            repo,
            branch,
            status: p.agent_status.as_deref().and_then(norm_status).unwrap_or("idle").into(),
            since: now.to_rfc3339(),
            tier: "push".into(),
            ..Default::default()
        });
    }

    // Addresses: `person/<dir>-<harness>`, suffixed -2, -3 when taken.
    let person = person.to_lowercase();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for s in &mut out {
        let slug: String = s.name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let mut name = slug.clone();
        let mut n = 2;
        while !taken.insert(name.clone()) {
            name = format!("{slug}-{n}");
            n += 1;
        }
        s.address = format!("{person}/{name}");
        s.name = name;
        s.machine = machine_id.clone();
        s.machine_name = machine_name.clone();
    }
    out
}

// ---------------------------------------------------------------- route

#[derive(Deserialize)]
struct SessionsQ {
    person: Option<String>,
}

/// `GET /api/factory/sessions[?person=<username>]` → `LiveSession[]` (C6). Mounted by the engine's factory router.
pub fn route<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/api/factory/sessions", get(sessions_h))
}

async fn sessions_h(Query(q): Query<SessionsQ>) -> Response {
    let person = q.person.filter(|p| !p.trim().is_empty()).unwrap_or_else(default_person);
    // File reads, git and the pane socket all block.
    match tokio::task::spawn_blocking(move || discover(&person, None)).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => crate::api::factory::error("internal", format!("session discovery failed: {e}"), "Retry; if it persists run `gizzi agents doctor`."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(lines: &[&str]) -> Seen {
        let mut s = Seen::default();
        for l in lines {
            s.visit(&serde_json::from_str(l).unwrap());
        }
        s
    }

    #[test]
    fn reads_titles_and_tasks_from_claude_and_codex() {
        let s = seen(&[
            r#"{"type":"ai-title","aiTitle":"Old"}"#,
            r#"{"type":"ai-title","aiTitle":"Auth refresh rework"}"#,
            r#"{"message":{"content":[{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"a","status":"completed"},{"content":"Run tests","status":"in_progress","activeForm":"Running tests"}]}}]}}"#,
        ]);
        assert_eq!(s.title.as_deref(), Some("Auth refresh rework"));
        assert_eq!(s.task, Some(Some("Running tests".into())));
        let s = seen(&[r#"{"payload":{"name":"update_plan","arguments":"{\"plan\":[{\"step\":\"Spawn\",\"status\":\"in_progress\"}]}"}}"#]);
        assert_eq!(s.task, Some(Some("Spawn".into())));
        let s = seen(&[r#"{"input":{"todos":[{"content":"a","status":"in_progress"}]}}"#, r#"{"input":{"todos":[{"content":"a","status":"completed"}]}}"#]);
        assert_eq!(s.task, Some(None));
    }

    #[test]
    fn scan_reads_appended_lines_only_and_clip_caps_length() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, "{\"type\":\"ai-title\",\"aiTitle\":\"One\"}\n").unwrap();
        assert_eq!(scan(&p).title.as_deref(), Some("One"));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "{{\"type\":\"ai-title\",\"aiTitle\":\"Two\"}}").unwrap();
        assert_eq!(scan(&p).title.as_deref(), Some("Two"));
        assert!(clip(&"x ".repeat(200)).chars().count() <= MAX_DOING);
    }

    #[test]
    fn pane_sessions_get_deduped_addresses() {
        let pane = |s: &str, id: &str, cwd: &str, st: &str| super::super::backend::LivePane {
            session: s.into(),
            pane_id: id.into(),
            cwd: Some(cwd.into()),
            agent_status: Some(st.into()),
        };
        let list = discover("Eoj", Some(vec![pane("ao-a", "p1", "/nonexistent/x/app", "working"), pane("ao-b", "p2", "/nonexistent/y/app", "idle")]));
        let mine: Vec<&LiveSession> = list.iter().filter(|s| s.pane_id.is_some()).collect();
        assert!(mine.iter().any(|s| s.address == "eoj/app-gizzi" && s.status == "busy" && s.tier == "push"));
        assert!(mine.iter().any(|s| s.address == "eoj/app-gizzi-2" && s.status == "idle"));
        assert_eq!(repo_slug("git@github.com:Allternit/allternit-platform.git").as_deref(), Some("Allternit/allternit-platform"));
    }
}

// ---------------------------------------------------------------- delivery source

/// The default session source for WP11's `live_delivery`: this machine's discovered
/// sessions, plus the push facts delivery needs (socket path, Codex thread, hooks).
pub struct Discovered;

impl Discovered {
    fn delivery_view(s: LiveSession) -> super::live_delivery::LiveSession {
        let socket = (s.harness == "claude")
            .then(|| s.pid)
            .flatten()
            .and_then(|pid| std::fs::read(claude_dir().join("sessions").join(format!("{pid}.json"))).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["messagingSocketPath"].as_str().filter(|p| !p.is_empty()).map(str::to_string));
        let waiting = super::live_delivery::is_waiting(&s.name);
        super::live_delivery::LiveSession {
            codex_thread: (s.harness == "codex").then(|| s.session_id.clone()),
            hooks_installed: s.tier == "turn" || read_hooks().iter().any(|h| h.session == s.session_id),
            messaging_socket_path: socket,
            messaging_token: None,
            waiting_inbox: waiting,
            address: s.address,
            name: s.name,
            harness: s.harness,
            machine: s.machine,
            machine_name: s.machine_name,
            pid: s.pid,
            session_id: s.session_id,
            pane_id: s.pane_id,
            bot_id: s.bot_id,
            cwd: s.cwd,
            repo: s.repo,
            branch: s.branch,
            status: s.status,
            since: s.since,
            doing: s.doing.unwrap_or_default(),
            tier: s.tier,
            unread: s.unread,
        }
    }
}

impl super::live_delivery::SessionLookup for Discovered {
    fn find(&self, address: &str) -> Option<super::live_delivery::LiveSession> {
        let want = address.to_lowercase();
        let name = want.rsplit('/').next().unwrap_or(&want).to_string();
        <LiveSession as SessionLookup>::all()
            .into_iter()
            .find(|s| s.address == want || (!want.contains('/') && s.name == name))
            .map(Self::delivery_view)
    }
    fn list(&self) -> Vec<super::live_delivery::LiveSession> {
        <LiveSession as SessionLookup>::all().into_iter().map(Self::delivery_view).collect()
    }
}
