//! Live sessions (contract C6): every agent session running on this computer,
//! with a no-token `doing` line read from what the agent records anyway.
//!
//! Sources, merged by working directory: Claude Code's `~/.claude/sessions/<pid>.json`
//! (dead pids skipped), Codex's `$CODEX_HOME/session_index.jsonl`, Factory panes
//! (`agent_status` from the pane engine) and the shared hooks file WP11 writes at
//! `~/.allternit/live-sessions/hooks.json`. Status: Claude's own `status` wins, then
//! the pane's `agent_status`, then hooks.
//!
//! Holders: the process table (read-only, [`super::proc_scan`]) adds the other harnesses'
//! processes (codex, kimi, grok, agy, gizzi, opencode, qwen), each mapped to its session id by
//! [`super::harness_table`]. Entries are grouped by `(harness, session id)`, so one session lists
//! every live pid holding it, the app each runs under, and whether it can move into Allternit
//! (`movable`). Nothing here signals or writes to another process.
//!
//! Portions adapted from Reason Network (reason-machines/reason-network), Apache-2.0. Copyright 2026 Reason Machines, Inc.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// `busy | idle | blocked | offline`. `blocked`: the agent is waiting on a person (a permission
    /// prompt or a question), as the pane engine detects it. Clients that only know `busy | idle`
    /// show the word as it is.
    pub status: String,
    pub since: String,
    /// When `status` turned `blocked` (RFC 3339). Left out unless the session is blocked; cleared
    /// as soon as it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_since: Option<String>,
    /// The Allternit terminal this session's pane is (start-anywhere's terminal id). Clients open it
    /// with the terminal thread view and type into it with `POST /terminal/<terminalId>/input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    /// How a client steers it: `terminal` (type into `terminalId` with the terminal input route),
    /// `delivery` (`POST /api/factory/send {to: address, text}`: the delivery ladder, with its rate
    /// limit and envelope) or `none` (nothing reaches it; open it on the computer). Empty from an
    /// older runtime: treat as unknown and hide steer controls.
    #[serde(default)]
    pub steer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// `push | turn | pull` (C1).
    pub tier: String,
    pub unread: u32,
    /// Every live process holding this session id (one writer per session id: two holders
    /// means it is open in two places). Empty for a pane with no agent process found.
    #[serde(default)]
    pub holders: Vec<Holder>,
    /// The flags the external holder was started with that a Move or Copy keeps
    /// (`--dangerously-skip-permissions`, `--model x`), in order.
    #[serde(default)]
    pub carried_flags: Vec<String>,
    /// The harness can fork this session into a new one (Copy).
    #[serde(default)]
    pub can_copy: bool,
    /// The exact argv a Move, Copy or New session would run (from [`super::harness_table`]),
    /// so clients show the command without knowing harness names. `None` when that mode is
    /// not offered. They hold `carried_flags`, so they stay on this computer like it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_command: Option<Vec<String>>,
    /// Whether it can move into Allternit: `now` (one external holder, idle, resumable),
    /// `busy` (mid-turn), `open_elsewhere` (more than one live holder), `no_resume` (the
    /// harness cannot resume by id, or no id was found), `here` (already in an Allternit pane).
    #[serde(default)]
    pub movable: String,
    /// A holder runs outside Allternit (Terminal, iTerm2, Ghostty, other).
    #[serde(default)]
    pub external: bool,
    /// The Move job for this session (`POST /api/v1/live-sessions/adopt`), kept 10 min after it ends.
    /// Filled in by allternit-api, which runs the jobs ([`super::adopt::annotate`]).
    #[serde(rename = "move", default, skip_serializing_if = "Option::is_none")]
    pub move_job: Option<super::adopt::MoveJob>,
}

/// One live process holding a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub pid: u32,
    /// `Terminal | iTerm2 | Allternit | Ghostty | other`, from the process's parent chain.
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The OS start time of the process (`ps -o lstart`; Claude Code's `procStart`), or RFC 3339
    /// when only a start timestamp is known. A pid whose start time changed is a different process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
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

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

// libc is a unix-only dependency; ask tasklist instead.
#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|out| out.contains(&format!("\"{pid}\"")))
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
        "blocked" => Some("blocked"),
        "idle" | "waiting" | "done" | "ready" => Some("idle"),
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
#[derive(Default)]
struct Found {
    harness: String,
    id: String,
    pid: Option<u32>,
    cwd: String,
    own_status: Option<String>,
    since: String,
    socket: bool,
    hook: Option<HookInfo>,
    /// The live processes holding it.
    procs: Vec<Proc>,
}

/// A live process holding a session, with what the listing needs about it.
#[derive(Clone, Default)]
struct Proc {
    holder: Holder,
    argv: Vec<String>,
    /// Runs under an Allternit pane (`Some(true)`), under something else (`Some(false)`),
    /// or not known (no process table).
    pane_held: Option<bool>,
}

/// A process that writes to its session store this recently is mid-turn.
const STORE_BUSY_SECS: u64 = 30;

/// `ps -o lstart` (`Sat Nov 21 03:55:44 2026`, local time) → RFC 3339.
fn lstart_rfc3339(s: &str) -> Option<String> {
    let t = chrono::NaiveDateTime::parse_from_str(s, "%a %b %d %H:%M:%S %Y").ok()?;
    chrono::Local.from_local_datetime(&t).single().map(|d| d.with_timezone(&Utc).to_rfc3339())
}

fn proc_of(pid: u32, started: Option<String>, argv: Option<Vec<String>>, table: &HashMap<u32, &super::proc_scan::ProcRow>) -> Proc {
    let row = table.get(&pid);
    let app = super::proc_scan::app_for(pid, table);
    Proc {
        holder: Holder {
            pid,
            app: app.unwrap_or("other").to_string(),
            tty: row.and_then(|r| r.tty.clone()),
            started_at: started.or_else(|| row.map(|r| r.started.clone())),
            ..Default::default()
        },
        argv: argv.or_else(|| super::proc_scan::argv(pid)).unwrap_or_default(),
        pane_held: app.map(|a| a == "Allternit"),
    }
}

/// One entry per `(harness, session id)`: two processes on one id become one session with two
/// holders. The representative is the pane-held entry if any, else the newest. Entries with no
/// id (a process whose session could not be found) are never merged.
fn group(found: Vec<Found>) -> Vec<Found> {
    let mut groups: Vec<Vec<Found>> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for f in found {
        if f.id.is_empty() {
            groups.push(vec![f]);
            continue;
        }
        match at.get(&(f.harness.clone(), f.id.clone())) {
            Some(&i) => groups[i].push(f),
            None => {
                at.insert((f.harness.clone(), f.id.clone()), groups.len());
                groups.push(vec![f]);
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|mut g| {
            if g.len() == 1 {
                return g.pop();
            }
            let rep = g.iter().position(|f| f.procs.iter().any(|p| p.pane_held == Some(true))).unwrap_or_else(|| {
                g.iter().enumerate().max_by(|a, b| a.1.since.cmp(&b.1.since)).map_or(0, |(i, _)| i)
            });
            let mut rep = g.swap_remove(rep);
            for f in g {
                for p in f.procs {
                    if !rep.procs.iter().any(|q| q.holder.pid == p.holder.pid) {
                        rep.procs.push(p);
                    }
                }
                if f.own_status.as_deref().and_then(norm_status) == Some("busy") {
                    rep.own_status = Some("busy".into());
                }
                rep.pid = rep.pid.or(f.pid);
                rep.hook = rep.hook.take().or(f.hook);
                if rep.cwd.is_empty() {
                    rep.cwd = f.cwd;
                }
            }
            Some(rep)
        })
        .collect()
}

/// What a listing says about moving a session (the new `LiveSession` fields).
#[derive(Debug, PartialEq)]
struct Moves {
    holders: Vec<Holder>,
    carried_flags: Vec<String>,
    can_copy: bool,
    movable: &'static str,
    external: bool,
    move_command: Option<Vec<String>>,
    copy_command: Option<Vec<String>>,
    new_command: Option<Vec<String>>,
}

/// `pane_id`: the Allternit pane this session was matched to. `unknown_holder`: a live process of
/// this harness outside Allternit has no known session id, so it may hold this one.
fn moves(f: &Found, pane_id: Option<&str>, status: &str, unknown_holder: bool) -> Moves {
    let holders: Vec<Holder> = f
        .procs
        .iter()
        .map(|p| {
            let mut h = p.holder.clone();
            if p.pane_held == Some(true) {
                h.pane_id = pane_id.map(str::to_string);
            }
            h
        })
        .collect();
    let external: Vec<&Proc> = f.procs.iter().filter(|p| p.holder.app != "Allternit").collect();
    let rep = external.iter().find(|p| Some(p.holder.pid) == f.pid).or_else(|| external.first());
    let carried_flags = rep.map(|p| super::harness_table::carried_flags(&f.harness, &p.argv)).unwrap_or_default();
    let movable = if holders.len() > 1 {
        "open_elsewhere"
    } else if holders.len() == 1 && holders[0].app == "Allternit" {
        "here"
    } else if holders.is_empty() && pane_id.is_some() {
        "here"
    } else if holders.is_empty() && unknown_holder {
        "open_elsewhere"
    } else if super::harness_table::move_argv(&f.harness, &f.id, &[]).is_none() {
        "no_resume"
    } else if status == "busy" {
        "busy"
    } else {
        "now"
    };
    // The commands shown before a click are the ones the adopt job runs (same table, same flags).
    let offered = !external.is_empty() && movable != "here";
    let move_command = (offered && matches!(movable, "now" | "busy"))
        .then(|| super::harness_table::move_argv(&f.harness, &f.id, &carried_flags))
        .flatten();
    let copy_command = offered.then(|| super::harness_table::copy_argv(&f.harness, &f.id, &carried_flags)).flatten();
    let new_command = offered.then(|| super::harness_table::new_argv(&f.harness, &carried_flags)).flatten();
    Moves {
        can_copy: super::harness_table::copy_argv(&f.harness, &f.id, &[]).is_some(),
        external: !external.is_empty(),
        holders,
        carried_flags,
        movable,
        move_command,
        copy_command,
        new_command,
    }
}

/// Harness processes outside Claude Code's pid registry (codex, kimi, grok, agy, gizzi,
/// opencode, qwen), each mapped to its session id where one can be found. `known` pids are
/// skipped. Returns the sessions and the harnesses with an external process whose session id
/// was not found.
fn scan_processes(
    rows: &[super::proc_scan::ProcRow],
    table: &HashMap<u32, &super::proc_scan::ProcRow>,
    known: &BTreeSet<u32>,
    panes: &[super::backend::LivePane],
    hooks: &[HookInfo],
) -> (Vec<Found>, BTreeSet<String>) {
    use super::harness_table as ht;
    let now = Utc::now();
    let mut out = Vec::new();
    let mut unresolved = BTreeSet::new();
    for row in rows.iter().filter(|r| r.tty.is_some() && !known.contains(&r.pid)) {
        let name = row.comm.rsplit('/').next().unwrap_or(&row.comm);
        let candidate = ht::HARNESSES.iter().any(|h| h.id != "claude" && h.procs.contains(&name)) || ht::INTERPRETERS.contains(&name);
        if !candidate {
            continue;
        }
        let raw = super::proc_scan::argv(row.pid).unwrap_or_default();
        let Some((h, argv)) = ht::harness_for_process(&row.comm, &raw) else { continue };
        // Claude Code comes from its pid registry; daemons and one-shot subcommands are not sessions.
        if h.id == "claude" || !ht::is_interactive(h, &argv) {
            continue;
        }
        let cwd = super::proc_scan::cwd(row.pid).unwrap_or_default();
        let p = proc_of(row.pid, None, Some(argv.clone()), table);
        let pane_held = p.pane_held == Some(true);
        let since = lstart_rfc3339(&row.started).unwrap_or_else(|| now.to_rfc3339());
        match ht::resolve_session(h.id, row.pid, &cwd, &argv) {
            Some(r) => {
                let recent = r.last_write.and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() <= STORE_BUSY_SECS);
                out.push(Found {
                    harness: h.id.into(),
                    hook: hooks.iter().find(|x| x.session == r.id).cloned(),
                    id: r.id,
                    pid: Some(row.pid),
                    cwd,
                    own_status: recent.then(|| "busy".to_string()),
                    since,
                    socket: false,
                    procs: vec![p],
                });
            }
            None => {
                // A pane-held process with no session id is already listed through its pane.
                if pane_held && panes.iter().any(|pn| !cwd.is_empty() && pn.cwd.as_deref() == Some(cwd.as_str())) {
                    continue;
                }
                if !pane_held {
                    unresolved.insert(h.id.to_string());
                }
                out.push(Found { harness: h.id.into(), pid: Some(row.pid), cwd, since, procs: vec![p], ..Default::default() });
            }
        }
    }
    (out, unresolved)
}

/// Every live session on this computer. `person` is the address prefix (the Clerk username);
/// `panes` are the Factory panes (pass `None` to ask the installed pane engine).
pub fn discover(person: &str, panes: Option<Vec<super::backend::LivePane>>) -> Vec<LiveSession> {
    discover_with(person, panes, false)
}

/// [`discover`] with each session's full working folder whatever `ALLTERNIT_SHARE_PATHS` says.
/// For this computer's own use only (adopt starts the moved session in that folder); never upload it.
pub fn discover_full_paths(person: &str, panes: Option<Vec<super::backend::LivePane>>) -> Vec<LiveSession> {
    discover_with(person, panes, true)
}

fn discover_with(person: &str, panes: Option<Vec<super::backend::LivePane>>, full_paths: bool) -> Vec<LiveSession> {
    // Terminal ids come from the same pane engine as the panes; injected panes (tests) bring none.
    let (panes, terminals) = match panes {
        Some(p) => (p, BTreeMap::new()),
        None => match super::backend::backend() {
            Ok(b) => (b.list().unwrap_or_default(), b.terminal_ids().unwrap_or_default()),
            Err(_) => (Vec::new(), BTreeMap::new()),
        },
    };
    discover_from(person, panes, &terminals, full_paths)
}

/// [`discover`] over a given pane list and pane id → terminal id map.
fn discover_from(person: &str, panes: Vec<super::backend::LivePane>, terminals: &BTreeMap<String, String>, full_paths: bool) -> Vec<LiveSession> {
    let share_paths = full_paths || matches!(std::env::var("ALLTERNIT_SHARE_PATHS").as_deref(), Ok("1" | "true"));
    let (machine_id, machine_name) = machine();
    let hooks = read_hooks();
    let now = Utc::now();
    let bots = pane_bots();
    let mut found: Vec<Found> = Vec::new();
    // The process table (read-only), for holders: which app each process runs under, its tty and start time.
    let rows = super::proc_scan::snapshot();
    let table = super::proc_scan::by_pid(&rows);

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
            // `procStart` is the OS start time of the pid; a later recheck compares it before acting on the pid.
            let started = v["procStart"].as_str().filter(|s| !s.is_empty()).map(str::to_string).or_else(|| v["startedAt"].as_i64().map(rfc3339_ms));
            found.push(Found {
                harness: "claude".into(),
                id: id.to_string(),
                pid: Some(pid),
                cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
                own_status: v["status"].as_str().map(str::to_string),
                since: rfc3339_ms(v["startedAt"].as_i64().unwrap_or_else(|| now.timestamp_millis())),
                socket: v["messagingSocketPath"].as_str().is_some_and(|s| !s.is_empty()),
                hook: hooks.iter().find(|h| h.session == id).cloned(),
                procs: vec![proc_of(pid, started, None, &table)],
            });
        }
    }

    // Codex: no pid, so a thread is live while its index entry is fresh. Its cwd comes from hooks.
    for (id, (_, at)) in codex_index() {
        let Some(at) = at.filter(|t| (now - *t).num_seconds() <= CODEX_LIVE_SECS) else { continue };
        let hook = hooks.iter().find(|h| h.session == id).cloned();
        let pid = hook.as_ref().and_then(|h| h.pid);
        found.push(Found {
            harness: "codex".into(),
            pid,
            cwd: hook.as_ref().and_then(|h| h.cwd.clone()).unwrap_or_default(),
            id,
            own_status: None,
            since: at.to_rfc3339(),
            socket: false,
            hook,
            procs: pid.filter(|p| pid_alive(*p)).map(|p| proc_of(p, None, None, &table)).into_iter().collect(),
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
            procs: h.pid.map(|p| proc_of(p, None, None, &table)).into_iter().collect(),
        });
    }

    // Other harnesses' processes (codex, kimi, grok, agy, gizzi, opencode, qwen), from the process table.
    let known: BTreeSet<u32> = found.iter().flat_map(|f| f.procs.iter().map(|p| p.holder.pid)).collect();
    let (scanned, unresolved) = scan_processes(&rows, &table, &known, &panes, &hooks);
    found.extend(scanned);
    // One entry per session id, every live pid a holder (decision 11: one writer per session id).
    let found = group(found);

    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<LiveSession> = Vec::new();
    for f in found {
        // A session held only by processes outside Allternit never claims a pane in the same folder.
        let may_claim_pane = f.procs.is_empty() || f.procs.iter().any(|p| p.pane_held != Some(false));
        let pane = panes
            .iter()
            .filter(|_| may_claim_pane)
            .find(|p| !used.contains(&p.pane_id) && !f.cwd.is_empty() && p.cwd.as_deref() == Some(f.cwd.as_str()));
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
        let m = moves(&f, pane.map(|p| p.pane_id.as_str()), status, unresolved.contains(&f.harness));
        let terminal_id = pane.and_then(|p| terminals.get(&p.pane_id)).cloned();
        let mut holders = m.holders;
        for h in holders.iter_mut().filter(|h| h.pane_id.is_some()) {
            h.terminal_id = terminal_id.clone();
        }
        out.push(LiveSession {
            steer: steer_for(terminal_id.as_deref(), status, pane.map(|p| p.session.as_str())).into(),
            bot_id: pane.and_then(|p| bots.get(&p.session).cloned()),
            terminal_id,
            holders,
            carried_flags: m.carried_flags,
            can_copy: m.can_copy,
            move_command: m.move_command,
            copy_command: m.copy_command,
            new_command: m.new_command,
            movable: m.movable.into(),
            external: m.external,
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
        let status = p.agent_status.as_deref().and_then(norm_status).unwrap_or("idle");
        let terminal_id = terminals.get(&p.pane_id).cloned();
        out.push(LiveSession {
            steer: steer_for(terminal_id.as_deref(), status, Some(p.session.as_str())).into(),
            bot_id: bots.get(&p.session).cloned(),
            terminal_id,
            name: format!("{}-{harness}", base_name(&cwd)),
            harness: harness.into(),
            session_id: p.session.clone(),
            pane_id: Some(p.pane_id.clone()),
            cwd: if share_paths { cwd } else { base_name(&cwd) },
            repo,
            branch,
            status: status.into(),
            since: now.to_rfc3339(),
            tier: "push".into(),
            // Already inside Allternit; the pane label is not a resumable session id.
            movable: "here".into(),
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
    {
        let mut since = blocked_log().lock().unwrap_or_else(|e| e.into_inner());
        for s in &mut out {
            s.blocked_since = stamp_blocked(&mut since, &blocked_key(s), &s.status, now);
        }
    }
    out
}

/// Pane session label -> bot id (`local:<slug>`), for panes the Factory registry says a bot owns.
/// `agents down <slug>` stops such a pane; clients read it from `botId`.
fn pane_bots() -> HashMap<String, String> {
    match super::registry::Registry::open_default().load() {
        Ok(file) => bots_of(&file),
        Err(_) => HashMap::new(),
    }
}

fn bots_of(file: &super::registry::RegistryFile) -> HashMap<String, String> {
    file.sessions
        .iter()
        .filter_map(|(session, e)| e.bot.as_ref().map(|b| (session.clone(), b.id.clone())))
        .collect()
}

/// A pane a person opened and started an agent in (not an `ao-` workspace): the pane engine
/// backend lists it as session `ao-pane-<paneId>`. It has no Factory agent record, so the
/// delivery ladder's pane path cannot reach it.
const ADOPTED_PANE_PREFIX: &str = "ao-pane-";

/// The `steer` field: the terminal route when the pane is an Allternit terminal, else the delivery
/// ladder, else nothing. `pane_session` is the pane's session label when the session sits in a pane.
fn steer_for(terminal_id: Option<&str>, status: &str, pane_session: Option<&str>) -> &'static str {
    if terminal_id.is_some() {
        "terminal"
    } else if status == "offline" || pane_session.is_some_and(|s| s.starts_with(ADOPTED_PANE_PREFIX)) {
        "none"
    } else {
        "delivery"
    }
}

/// When each blocked session turned blocked, by [`blocked_key`], across discovery runs.
fn blocked_log() -> &'static Mutex<HashMap<String, String>> {
    static LOG: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LOG.get_or_init(Default::default)
}

/// The pane when there is one (a pane outlives the session id it shows), else harness + session id.
fn blocked_key(s: &LiveSession) -> String {
    match &s.pane_id {
        Some(p) => format!("pane:{p}"),
        None if !s.session_id.is_empty() => format!("{}:{}", s.harness, s.session_id),
        None => format!("addr:{}", s.address),
    }
}

/// Most entries kept. A run that would pass it starts the log over (stamps restart); entries
/// leave as soon as their session is seen not blocked, so this only bounds sessions that vanished while blocked.
const MAX_BLOCKED_LOG: usize = 512;

/// `blockedSince` for one session: kept from the first run that saw it blocked, cleared by the
/// first run that sees it any other way.
fn stamp_blocked(log: &mut HashMap<String, String>, key: &str, status: &str, now: DateTime<Utc>) -> Option<String> {
    if status != "blocked" {
        log.remove(key);
        return None;
    }
    if log.len() >= MAX_BLOCKED_LOG && !log.contains_key(key) {
        log.clear();
    }
    Some(log.entry(key.to_string()).or_insert_with(|| now.to_rfc3339()).clone())
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

    fn live_pane(session: &str, id: &str, cwd: &str, status: &str) -> super::super::backend::LivePane {
        super::super::backend::LivePane { session: session.into(), pane_id: id.into(), cwd: Some(cwd.into()), agent_status: Some(status.into()) }
    }

    #[test]
    fn registered_bot_panes_get_bot_id_and_others_do_not() {
        let mut file = super::super::registry::RegistryFile::default();
        file.sessions.insert(
            "ao-lead-bonsai-lead".into(),
            super::super::registry::Entry {
                bot: Some(super::super::registry::BotRef::placeholder("lead-bonsai-lead")),
                ..Default::default()
            },
        );
        let bots = bots_of(&file);
        assert_eq!(bots.get("ao-lead-bonsai-lead").map(String::as_str), Some("local:lead-bonsai-lead"));
        assert_eq!(bots.get("ao-pane-w3:p1"), None);
    }

    #[test]
    fn blocked_pane_stays_blocked_with_blocked_since() {
        let panes = vec![live_pane("ao-blk", "p-blk-1", "/nonexistent/blk/app", "blocked"), live_pane("ao-wrk", "p-blk-2", "/nonexistent/blk/other", "working")];
        let list = discover_from("eoj", panes, &BTreeMap::new(), false);
        let blocked = list.iter().find(|s| s.pane_id.as_deref() == Some("p-blk-1")).unwrap();
        assert_eq!(blocked.status, "blocked");
        let since = blocked.blocked_since.clone().expect("blockedSince set while blocked");
        assert!(DateTime::parse_from_rfc3339(&since).is_ok());
        let working = list.iter().find(|s| s.pane_id.as_deref() == Some("p-blk-2")).unwrap();
        assert_eq!((working.status.as_str(), working.blocked_since.as_deref()), ("busy", None));
        // The next run keeps the first stamp; once it is idle the stamp goes.
        let again = discover_from("eoj", vec![live_pane("ao-blk", "p-blk-1", "/nonexistent/blk/app", "blocked")], &BTreeMap::new(), false);
        assert_eq!(again.iter().find(|s| s.pane_id.as_deref() == Some("p-blk-1")).unwrap().blocked_since.as_deref(), Some(since.as_str()));
        let idle = discover_from("eoj", vec![live_pane("ao-blk", "p-blk-1", "/nonexistent/blk/app", "idle")], &BTreeMap::new(), false);
        let row = idle.iter().find(|s| s.pane_id.as_deref() == Some("p-blk-1")).unwrap();
        assert_eq!((row.status.as_str(), row.blocked_since.as_deref()), ("idle", None));
    }

    #[test]
    fn blocked_since_is_set_on_block_and_cleared_otherwise() {
        let mut log = HashMap::new();
        let t0 = Utc.with_ymd_and_hms(2026, 11, 27, 9, 0, 0).unwrap();
        let t1 = t0 + chrono::Duration::minutes(5);
        assert_eq!(stamp_blocked(&mut log, "pane:p1", "blocked", t0), Some(t0.to_rfc3339()));
        assert_eq!(stamp_blocked(&mut log, "pane:p1", "blocked", t1), Some(t0.to_rfc3339()));
        assert_eq!(stamp_blocked(&mut log, "pane:p1", "busy", t1), None);
        assert_eq!(stamp_blocked(&mut log, "pane:p1", "blocked", t1), Some(t1.to_rfc3339()));
        assert_eq!(stamp_blocked(&mut log, "pane:p1", "idle", t1), None);
        assert!(log.is_empty());
    }

    #[test]
    fn pane_steer_uses_the_terminal_route_when_the_pane_is_a_terminal() {
        let terminals = BTreeMap::from([("p-st-1".to_string(), "term_7c1e9a40".to_string())]);
        let panes = vec![
            live_pane("ao-pane-p-st-1", "p-st-1", "/nonexistent/st/a", "blocked"),
            live_pane("ao-pane-p-st-2", "p-st-2", "/nonexistent/st/b", "idle"),
            live_pane("ao-build", "p-st-3", "/nonexistent/st/c", "idle"),
        ];
        let list = discover_from("eoj", panes, &terminals, false);
        let row = |id: &str| list.iter().find(|s| s.pane_id.as_deref() == Some(id)).unwrap();
        assert_eq!((row("p-st-1").steer.as_str(), row("p-st-1").terminal_id.as_deref()), ("terminal", Some("term_7c1e9a40")));
        // A pane someone opened, not a terminal: nothing reaches it from another device.
        assert_eq!((row("p-st-2").steer.as_str(), row("p-st-2").terminal_id.as_deref()), ("none", None));
        // A Factory agent pane: the delivery ladder (`POST /api/factory/send`).
        assert_eq!(row("p-st-3").steer, "delivery");
        assert_eq!(steer_for(None, "offline", None), "none");
    }

    #[test]
    fn contract_fixture_reads_and_round_trips() {
        let raw = include_str!("../../../../docs/contracts/live-sessions/session.pane.blocked.json");
        let s: LiveSession = serde_json::from_str(raw).unwrap();
        assert_eq!((s.status.as_str(), s.steer.as_str(), s.terminal_id.as_deref()), ("blocked", "terminal", Some("term_7c1e9a40")));
        assert!(s.blocked_since.is_some());
        let back = serde_json::to_value(&s).unwrap();
        for key in ["terminalId", "steer", "blockedSince"] {
            assert!(back.get(key).is_some(), "{key} serialized");
        }
        // An older runtime's row (no new fields) still reads.
        let old: LiveSession = serde_json::from_str(include_str!("../../../../docs/contracts/live-sessions/session.pane.legacy.json")).unwrap();
        assert_eq!((old.steer.as_str(), old.terminal_id, old.blocked_since), ("", None, None));
    }

    fn held(harness: &str, id: &str, pid: u32, app: &str, since: &str, argv: &[&str]) -> Found {
        Found {
            harness: harness.into(),
            id: id.into(),
            pid: Some(pid),
            cwd: "/repo".into(),
            since: since.into(),
            procs: vec![Proc {
                holder: Holder { pid, app: app.into(), started_at: Some("Sat Nov 21 03:55:44 2026".into()), ..Default::default() },
                argv: argv.iter().map(|s| s.to_string()).collect(),
                pane_held: Some(app == "Allternit"),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn live_sessions_holders_two_pids_on_one_session_are_one_entry_open_elsewhere() {
        let list = group(vec![
            held("claude", "s1", 101, "Terminal", "2026-11-21T03:00:00+00:00", &["claude", "--dangerously-skip-permissions"]),
            held("claude", "other", 150, "Terminal", "2026-11-21T03:30:00+00:00", &["claude"]),
            held("claude", "s1", 202, "iTerm2", "2026-11-21T04:00:00+00:00", &["claude", "--resume", "s1", "--model", "opus"]),
        ]);
        assert_eq!(list.len(), 2);
        let s1 = list.iter().find(|f| f.id == "s1").unwrap();
        // The newest entry represents the session; both pids are holders.
        assert_eq!(s1.pid, Some(202));
        assert_eq!(s1.procs.iter().map(|p| p.holder.pid).collect::<Vec<_>>(), vec![202, 101]);
        let m = moves(s1, None, "idle", false);
        assert_eq!(m.movable, "open_elsewhere");
        assert_eq!(m.holders.len(), 2);
        assert!(m.external && m.can_copy);
        assert_eq!(m.carried_flags, vec!["--model", "opus"]);
    }

    #[test]
    fn live_sessions_holders_movable_per_case() {
        let idle = held("claude", "s1", 101, "Terminal", "", &["claude", "--dangerously-skip-permissions"]);
        let m = moves(&idle, None, "idle", false);
        assert_eq!((m.movable, m.external, m.carried_flags.clone()), ("now", true, vec!["--dangerously-skip-permissions".to_string()]));
        // The command shown before a Move is the one the adopt job runs.
        assert!(m.move_command.is_some());
        assert_eq!(m.move_command, crate::agents::harness_table::move_argv("claude", "s1", &m.carried_flags));
        assert_eq!(m.new_command, crate::agents::harness_table::new_argv("claude", &m.carried_flags));
        assert_eq!(moves(&idle, None, "busy", false).movable, "busy");
        // Only inside an Allternit pane: already here; its pane id goes on the holder.
        let pane = held("claude", "s2", 300, "Allternit", "", &["claude"]);
        let m = moves(&pane, Some("w1:p2"), "idle", false);
        assert_eq!((m.movable, m.external), ("here", false));
        assert_eq!(m.holders[0].pane_id.as_deref(), Some("w1:p2"));
        assert_eq!((m.move_command, m.copy_command, m.new_command), (None, None, None));
        assert!(m.carried_flags.is_empty());
        // A pane holder plus a Terminal holder (the 11-20 12:03 incident): refused, and the pane entry represents it.
        let both = group(vec![held("claude", "s3", 400, "Terminal", "2026-11-21T09:00:00+00:00", &["claude"]), held("claude", "s3", 401, "Allternit", "2026-11-21T01:00:00+00:00", &["claude"])]);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].pid, Some(401));
        assert_eq!(moves(&both[0], None, "idle", false).movable, "open_elsewhere");
        // qwen resumes by the id in its argv (`--resume`); with no id found it cannot.
        assert_eq!(moves(&held("qwen", "q1", 500, "Terminal", "", &["qwen"]), None, "idle", false).movable, "now");
        assert_eq!(moves(&held("qwen", "", 501, "Terminal", "", &["qwen"]), None, "idle", false).movable, "no_resume");
        let unknown = held("codex", "", 600, "Ghostty", "", &["codex"]);
        let m = moves(&unknown, None, "idle", false);
        assert_eq!((m.movable, m.can_copy), ("no_resume", false));
        // Two processes with no id are never merged into one.
        assert_eq!(group(vec![unknown, held("codex", "", 601, "Terminal", "", &["codex"])]).len(), 2);
        // A live codex thread with no process found while an unidentified codex process runs: it may hold it.
        let orphan = Found { harness: "codex".into(), id: "u1".into(), ..Default::default() };
        assert_eq!(moves(&orphan, None, "idle", true).movable, "open_elsewhere");
        assert_eq!(moves(&orphan, None, "idle", false).movable, "now");
        assert!(lstart_rfc3339("Sun Nov 1 03:55:44 2026").is_some());
        assert!(lstart_rfc3339("not a date").is_none());
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
        // The token `gizzi team-mcp` registered from CLAUDE_CODE_MESSAGING_TOKEN (and the
        // socket, when the session file lacks it).
        let registered = (s.harness == "claude").then(|| super::live_receipts::messaging(s.pid, &s.session_id)).flatten();
        let socket = socket.or_else(|| registered.as_ref().map(|m| m.socket.clone()).filter(|p| !p.is_empty() && s.status != "offline"));
        let cwd = if s.cwd.is_empty() { registered.as_ref().map(|m| m.cwd.clone()).unwrap_or_default() } else { s.cwd.clone() };
        let waiting = super::live_delivery::is_waiting(&s.name);
        super::live_delivery::LiveSession {
            codex_thread: (s.harness == "codex").then(|| s.session_id.clone()),
            hooks_installed: s.tier == "turn" || read_hooks().iter().any(|h| h.session == s.session_id),
            messaging_socket_path: socket,
            messaging_token: registered.map(|m| m.token).filter(|t| !t.is_empty()),
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
            cwd,
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
