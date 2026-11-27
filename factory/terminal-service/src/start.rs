//! Session start ("Start anywhere", contract in `docs/contracts/session-start/`): what a computer
//! offers a phone or another session (`GET /api/v1/session-start/options`) and the plan for one
//! start (`POST /api/v1/session-start`). Pure library: no HTTP, no pane engine calls. The caller
//! (allternit-api) mounts the routes, runs the plan and sends the JSON these types serialize to.
//!
//! - [`build_options`] gathers up to 30 newest folders with git facts (`git -C`, short timeout)
//!   and, per runtime, the permission mode the CLI's own config starts in and the modes it has
//!   flags for.
//! - [`plan_start`] checks one [`StartRequest`] (safety switch, mode cap, bypass, depth, runtime,
//!   folder) and returns a [`StartPlan`]: an engine session (runtime `allternit`, Gizzi) or a
//!   pane terminal running the CLI.
//! - [`StartGuards`] adds the per-device rate limit (6 starts a minute) and the `Idempotency-Key`
//!   replay (10 minutes) around it.
//!
//! The per-CLI flags live in [`crate::harness_modes`] (`mode_flags`, `start_argv`), the one table
//! allternit-api, allternit-node and the Factory harness table all read.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::harness_modes::{mode_flags, start_argv};

/// At most this many folders in the options.
pub const MAX_FOLDERS: usize = 30;
/// Sessions started from sessions go at most this deep.
pub const MAX_DEPTH: u32 = 3;
/// Starts a device accepts per minute.
pub const STARTS_PER_MINUTE: u64 = 6;
/// How long an `Idempotency-Key` replays its first result.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(600);
/// The engine runtime (Gizzi). Every other runtime id is a CLI in a terminal.
pub const ENGINE_RUNTIME: &str = "allternit";
const GIT_TIMEOUT: Duration = Duration::from_millis(1500);

// ───────────────────────────────────────────────────────────────── modes

/// The app's permission modes (`ChatPermissionPill` ids plus `bypass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Auto,
    Default,
    Plan,
    Bypass,
}

impl Mode {
    /// Listed in this order in `supported`.
    pub const ALL: [Mode; 4] = [Mode::Auto, Mode::Default, Mode::Plan, Mode::Bypass];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Default => "default",
            Mode::Plan => "plan",
            Mode::Bypass => "bypass",
        }
    }

    /// How much a mode lets the agent do on its own: plan < default < auto < bypass.
    pub fn rank(self) -> u8 {
        match self {
            Mode::Plan => 0,
            Mode::Default => 1,
            Mode::Auto => 2,
            Mode::Bypass => 3,
        }
    }
}

// ───────────────────────────────────────────────────────────────── options

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDevice {
    pub runtime_id: String,
    pub name: String,
    pub os: String,
    /// False when the safety switch is off on this computer.
    pub remote_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    /// `origin`'s URL with any `user:token@` removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// `None` on a detached HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub head: String,
    pub dirty: bool,
    /// Commits ahead of the upstream (0 without one).
    pub ahead: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartFolder {
    pub path: String,
    pub name: String,
    /// RFC 3339, UTC, seconds (`2026-11-27T15:42:10Z`).
    pub last_used_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeModes {
    /// The mode the CLI's own config starts in on this computer.
    pub default: Mode,
    pub supported: Vec<Mode>,
}

/// `GET /api/v1/session-start/options` (`options.response.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOptions {
    pub device: StartDevice,
    pub folders: Vec<StartFolder>,
    pub modes: BTreeMap<String, RuntimeModes>,
    pub default_runtime: String,
}

/// A folder the caller knows about: Gizzi projects, live-session and terminal cwds, home.
#[derive(Debug, Clone)]
pub struct FolderCandidate {
    pub path: PathBuf,
    /// When it was last used; the folder's mtime when unknown.
    pub last_used: Option<SystemTime>,
}

pub struct OptionsInput {
    pub device: StartDevice,
    pub candidates: Vec<FolderCandidate>,
    /// Installed runtime ids (registry ids: `claude`, `codex`, …). `allternit` is always listed.
    pub runtimes: Vec<String>,
    /// Home folder the CLIs' configs are read from.
    pub home: PathBuf,
}

/// The options a computer offers. Runs `git` up to twice per folder, in parallel, each capped
/// at 1.5 s.
pub fn build_options(input: OptionsInput) -> StartOptions {
    let mut modes = BTreeMap::new();
    modes.insert(
        ENGINE_RUNTIME.to_string(),
        RuntimeModes {
            default: device_default_mode(ENGINE_RUNTIME, &input.home),
            supported: Mode::ALL.to_vec(),
        },
    );
    for rt in &input.runtimes {
        if rt == ENGINE_RUNTIME || modes.contains_key(rt) {
            continue;
        }
        let supported = Mode::ALL
            .iter()
            .copied()
            .filter(|m| mode_flags(rt, m.as_str()).is_some())
            .collect();
        modes.insert(
            rt.clone(),
            RuntimeModes {
                default: device_default_mode(rt, &input.home),
                supported,
            },
        );
    }
    StartOptions {
        device: input.device,
        folders: collect_folders(input.candidates),
        modes,
        default_runtime: ENGINE_RUNTIME.to_string(),
    }
}

/// Existing folders, one per path, newest first, at most [`MAX_FOLDERS`], with git facts.
pub fn collect_folders(candidates: Vec<FolderCandidate>) -> Vec<StartFolder> {
    let mut newest: HashMap<PathBuf, SystemTime> = HashMap::new();
    for c in candidates {
        if !c.path.is_absolute() || !c.path.is_dir() {
            continue;
        }
        let path = std::fs::canonicalize(&c.path).unwrap_or(c.path);
        let at = c
            .last_used
            .or_else(|| std::fs::metadata(&path).and_then(|m| m.modified()).ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let e = newest.entry(path).or_insert(at);
        if at > *e {
            *e = at;
        }
    }
    let mut list: Vec<(PathBuf, SystemTime)> = newest.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list.truncate(MAX_FOLDERS);
    std::thread::scope(|s| {
        let handles: Vec<_> = list
            .iter()
            .map(|(p, _)| s.spawn(move || git_info(p)))
            .collect();
        list.iter()
            .zip(handles)
            .map(|((path, at), h)| StartFolder {
                path: path.to_string_lossy().into_owned(),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned()),
                last_used_at: rfc3339(*at),
                git: h.join().ok().flatten(),
            })
            .collect()
    })
}

fn rfc3339(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Runs `git -C <dir> <args>`; `None` on a non-zero exit, a missing git or the timeout.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on a thread so a big `status` cannot fill the pipe and stall the child.
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    status.filter(|s| s.success()).map(|_| text)
}

/// Git facts for a folder: `None` when it is not a repository or has no commit yet.
pub fn git_info(dir: &Path) -> Option<GitInfo> {
    let status = git(dir, &["status", "--porcelain=v2", "--branch"])?;
    let mut info = parse_status(&status)?;
    info.remote = git(dir, &["config", "--get", "remote.origin.url"])
        .map(|u| strip_credentials(u.trim()))
        .filter(|u| !u.is_empty());
    Some(info)
}

fn parse_status(status: &str) -> Option<GitInfo> {
    let (mut head, mut branch, mut ahead, mut dirty) = (None, None, 0u32, false);
    for line in status.lines() {
        if let Some(v) = line.strip_prefix("# branch.oid ") {
            head = (v != "(initial)").then(|| v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("# branch.head ") {
            branch = (v != "(detached)").then(|| v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("# branch.ab ") {
            ahead = v
                .split_whitespace()
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            dirty = true;
        }
    }
    Some(GitInfo {
        remote: None,
        branch,
        head: head?,
        dirty,
        ahead,
    })
}

/// `https://user:token@host/x.git` → `https://host/x.git`. scp-style `git@host:x` stays.
pub fn strip_credentials(url: &str) -> String {
    let Some(i) = url.find("://") else {
        return url.to_string();
    };
    let (scheme, rest) = url.split_at(i + 3);
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// `host/path` for comparing remotes written as https, ssh or scp (`git@host:org/x.git`).
fn remote_key(url: &str) -> String {
    let url = strip_credentials(url.trim());
    let rest = match url.find("://") {
        Some(i) => url[i + 3..].to_string(),
        None => match url.split_once(':') {
            Some((host, path)) => format!("{}/{}", host.rsplit('@').next().unwrap_or(host), path),
            None => url.clone(),
        },
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest.as_str(), ""));
    let host = host
        .rsplit('@')
        .next()
        .unwrap_or(host)
        .split(':')
        .next()
        .unwrap_or(host);
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{}/{}", host, path).to_ascii_lowercase()
}

/// The mode a CLI starts in on this computer, read from its own config; `default` when it has
/// none. claude: `~/.claude/settings.json` `permissions.defaultMode`. codex:
/// `~/.codex/config.toml` top-level `approval_policy` / `sandbox_mode` (with neither set, codex
/// starts in its Auto preset: workspace-write, on-request).
pub fn device_default_mode(runtime_id: &str, home: &Path) -> Mode {
    let id = runtime_id.trim().to_ascii_lowercase();
    match id.strip_suffix("-cli").unwrap_or(id.as_str()) {
        "claude" => {
            let mode = std::fs::read_to_string(home.join(".claude/settings.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| {
                    v.pointer("/permissions/defaultMode")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            match mode.as_deref() {
                Some("acceptEdits") | Some("auto") => Mode::Auto,
                Some("plan") => Mode::Plan,
                Some("bypassPermissions") => Mode::Bypass,
                _ => Mode::Default,
            }
        }
        "codex" => {
            let Ok(text) = std::fs::read_to_string(home.join(".codex/config.toml")) else {
                return Mode::Default;
            };
            let approval = toml_top_level(&text, "approval_policy");
            let sandbox = toml_top_level(&text, "sandbox_mode");
            match (approval.as_deref(), sandbox.as_deref()) {
                (Some("never"), Some("danger-full-access")) => Mode::Bypass,
                (_, Some("read-only")) | (Some("untrusted"), _) => Mode::Default,
                _ => Mode::Auto,
            }
        }
        _ => Mode::Default,
    }
}

/// A top-level `key = "value"` from a TOML file (before the first `[table]`).
fn toml_top_level(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            return None;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        let v = match v.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(""),
            None => v.split('#').next().unwrap_or("").trim(),
        };
        return Some(v.to_string());
    }
    None
}

// ───────────────────────────────────────────────────────────────── request

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRef {
    pub remote: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// `{ "path": … }` (a folder on this computer) or `{ "git": { remote, branch, head? } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FolderRef {
    Path { path: String },
    Git { git: GitRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOrigin {
    /// `web`, `phone`, `pwa`, `ios`, `android`, `desktop`, `gizzi`, …
    pub surface: String,
    pub device_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Depth of the new session: 0 for a person's start, parent's depth + 1 from a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `POST /api/v1/session-start` body (`start.request.*.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub folder: FolderRef,
    pub runtime: String,
    /// `None`: this computer's default for that runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub confirm_bypass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub origin: StartOrigin,
}

/// `201` body (`start.response.*.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StartResponse {
    Engine {
        #[serde(rename = "sessionId")]
        session_id: String,
        directory: String,
    },
    Terminal {
        #[serde(rename = "terminalId")]
        terminal_id: String,
        cwd: String,
        agent: String,
    },
}

/// What the caller does to start the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPlan {
    /// Create a Gizzi engine session with `directory` and `mode` (`POST /v1/session`), then send
    /// `prompt` as its first message.
    Engine {
        directory: PathBuf,
        mode: Mode,
        prompt: Option<String>,
        origin: StartOrigin,
    },
    /// Open a pane terminal running `argv` in `cwd` (`agent_request`). `send_after_start`: the
    /// prompt, when the CLI takes none on its command line; type it in once the CLI is up.
    Terminal {
        argv: Vec<String>,
        cwd: PathBuf,
        title: String,
        agent: String,
        send_after_start: Option<String>,
    },
}

// ───────────────────────────────────────────────────────────────── errors

/// `{status, error, message, action}` exactly as `errors.json`. The HTTP body is [`StartError::body`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StartError {
    pub status: u16,
    pub error: &'static str,
    pub message: &'static str,
    pub action: &'static str,
}

impl StartError {
    pub const REMOTE_START_OFF: StartError = StartError {
        status: 403,
        error: "remote_start_off",
        message: "Remote start is turned off on this computer.",
        action: "Turn on Remote start in Settings on that computer, or start the session there.",
    };
    pub const MODE_NOT_ALLOWED: StartError = StartError {
        status: 403,
        error: "mode_not_allowed",
        message: "This permission mode is higher than the one you are allowed to grant.",
        action: "Pick a mode at or below your own, or confirm bypass on the computer.",
    };
    pub const DEPTH_EXCEEDED: StartError = StartError {
        status: 403,
        error: "depth_exceeded",
        message: "Sessions started from sessions can only go three levels deep.",
        action: "Start the work from a top-level session instead.",
    };
    pub const FOLDER_MISSING: StartError = StartError {
        status: 404,
        error: "folder_missing",
        message: "That folder no longer exists on this computer.",
        action: "Pick another folder from the list.",
    };
    pub const RUNTIME_MISSING: StartError = StartError {
        status: 404,
        error: "runtime_missing",
        message: "That coding tool is not installed on this computer.",
        action: "Pick another tool or install it on that computer.",
    };
    pub const RUNTIME_SIGNED_OUT: StartError = StartError {
        status: 409,
        error: "runtime_signed_out",
        message: "That coding tool is signed out on this computer.",
        action: "Sign in to the tool on that computer, then try again.",
    };
    pub const RATE_LIMITED: StartError = StartError {
        status: 429,
        error: "rate_limited",
        message: "This computer has started too many sessions in the last minute.",
        action: "Wait a minute, then try again.",
    };
    pub const CLONE_FAILED: StartError = StartError {
        status: 502,
        error: "clone_failed",
        message: "The cloud computer could not copy the repository.",
        action: "Check that the repository is reachable, then try again.",
    };
    pub const ALL: [StartError; 8] = [
        Self::REMOTE_START_OFF,
        Self::MODE_NOT_ALLOWED,
        Self::DEPTH_EXCEEDED,
        Self::FOLDER_MISSING,
        Self::RUNTIME_MISSING,
        Self::RUNTIME_SIGNED_OUT,
        Self::RATE_LIMITED,
        Self::CLONE_FAILED,
    ];

    /// The response body: `{error, message, action}`.
    pub fn body(&self) -> Value {
        json!({ "error": self.error, "message": self.message, "action": self.action })
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error, self.message)
    }
}

impl std::error::Error for StartError {}

// ───────────────────────────────────────────────────────────────── planning

/// What the computer knows when it plans a start.
pub struct StartCtx<'a> {
    /// The safety switch (D6). Off refuses relayed starts.
    pub remote_start_enabled: bool,
    /// The request came through the cloud relay (`RelayedAuth`), not from this computer.
    pub relayed: bool,
    /// The starting session's own mode (StartSession tool); `None` for a person's start.
    pub caller_mode: Option<Mode>,
    /// Installed runtime ids; includes `allternit` when the engine is available.
    pub installed: &'a HashSet<String>,
    /// Installed runtimes known to be signed out.
    pub signed_out: &'a HashSet<String>,
    /// Per-runtime modes, as in [`StartOptions::modes`].
    pub modes: &'a BTreeMap<String, RuntimeModes>,
    /// Known folders (for `folder.git` when there is no resolver), newest first.
    pub folders: &'a [StartFolder],
    /// Turns `folder.git` into a folder (a cloud computer clones; `Err(CLONE_FAILED)` on failure).
    /// `None`: match a known folder with the same remote and branch.
    pub git_resolver: Option<&'a dyn Fn(&GitRef) -> Result<PathBuf, StartError>>,
}

/// Checks a start and says how to run it. Stateless: rate limit and idempotency are
/// [`StartGuards::plan`].
pub fn plan_start(req: &StartRequest, ctx: &StartCtx) -> Result<StartPlan, StartError> {
    if ctx.relayed && !ctx.remote_start_enabled {
        return Err(StartError::REMOTE_START_OFF);
    }
    if req.origin.depth.unwrap_or(0) > MAX_DEPTH {
        return Err(StartError::DEPTH_EXCEEDED);
    }
    let runtime = req.runtime.as_str();
    let engine = runtime == ENGINE_RUNTIME;
    if !ctx.installed.contains(runtime)
        || (!engine && start_argv(runtime, None, None).is_none())
    {
        return Err(StartError::RUNTIME_MISSING);
    }
    if ctx.signed_out.contains(runtime) {
        return Err(StartError::RUNTIME_SIGNED_OUT);
    }

    // Mode: the asked one, else this computer's default for the runtime.
    let device_default = ctx
        .modes
        .get(runtime)
        .map(|m| m.default)
        .unwrap_or(Mode::Default);
    let mode = req.mode.unwrap_or(device_default);
    if let Some(asked) = req.mode {
        let flags_exist = engine || start_argv(runtime, Some(asked.as_str()), None).is_some();
        if !flags_exist {
            return Err(StartError::MODE_NOT_ALLOWED);
        }
    }
    if ctx.caller_mode.is_some_and(|c| mode.rank() > c.rank()) {
        return Err(StartError::MODE_NOT_ALLOWED);
    }
    if mode == Mode::Bypass && ctx.relayed && device_default != Mode::Bypass && !req.confirm_bypass
    {
        return Err(StartError::MODE_NOT_ALLOWED);
    }

    let dir = resolve_folder(&req.folder, ctx)?;
    let prompt = req.prompt.clone().filter(|p| !p.trim().is_empty());
    if engine {
        return Ok(StartPlan::Engine {
            directory: dir,
            mode,
            prompt,
            origin: req.origin.clone(),
        });
    }
    // No asked mode: start with no mode flags, so the CLI uses its own config (= the device default).
    let (argv, prompt_in_argv) =
        start_argv(runtime, req.mode.map(Mode::as_str), prompt.as_deref())
            .ok_or(StartError::MODE_NOT_ALLOWED)?;
    let folder_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned());
    Ok(StartPlan::Terminal {
        argv,
        title: format!("{runtime} · {folder_name}"),
        cwd: dir,
        agent: runtime.to_string(),
        send_after_start: if prompt_in_argv { None } else { prompt },
    })
}

fn resolve_folder(folder: &FolderRef, ctx: &StartCtx) -> Result<PathBuf, StartError> {
    match folder {
        FolderRef::Path { path } => {
            let p = PathBuf::from(path);
            if p.is_absolute() && p.is_dir() {
                Ok(p)
            } else {
                Err(StartError::FOLDER_MISSING)
            }
        }
        FolderRef::Git { git } => {
            if let Some(resolve) = ctx.git_resolver {
                return resolve(git);
            }
            let want = remote_key(&git.remote);
            ctx.folders
                .iter()
                .filter(|f| {
                    f.git.as_ref().is_some_and(|g| {
                        g.branch.as_deref() == Some(git.branch.as_str())
                            && g.remote.as_deref().is_some_and(|r| remote_key(r) == want)
                    })
                })
                .map(|f| PathBuf::from(&f.path))
                .find(|p| p.is_dir())
                .ok_or(StartError::FOLDER_MISSING)
        }
    }
}

// ───────────────────────────────────────────────────────────────── rate limit + idempotency

/// Monotonic time since some fixed start. Tests pass a fake one.
pub type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

const MINUTE_MS: u64 = 60_000;
const START_COST_MS: u64 = MINUTE_MS / STARTS_PER_MINUTE;

/// Either a replay of the first result for this `Idempotency-Key`, or a new plan to run.
#[derive(Debug, Clone, PartialEq)]
pub enum Planned {
    Replay(Value),
    Fresh(StartPlan),
}

#[derive(Default)]
struct GuardState {
    /// Per device: credit in ms; a start costs 10 s of credit, the cap is one minute (6 starts).
    buckets: HashMap<String, (u64, u64)>,
    /// `device \0 key` -> (stored at ms, response).
    replies: HashMap<String, (u64, Value)>,
}

/// In-memory, per process: the device's own 6-starts-a-minute bucket and the idempotency map.
pub struct StartGuards {
    clock: Clock,
    state: Mutex<GuardState>,
}

impl Default for StartGuards {
    fn default() -> Self {
        Self::new()
    }
}

impl StartGuards {
    pub fn new() -> Self {
        let t0 = Instant::now();
        Self::with_clock(Arc::new(move || t0.elapsed()))
    }

    pub fn with_clock(clock: Clock) -> Self {
        Self {
            clock,
            state: Mutex::new(GuardState::default()),
        }
    }

    fn now_ms(&self) -> u64 {
        (self.clock)().as_millis() as u64
    }

    fn reply_key(device_id: &str, key: &str) -> String {
        format!("{device_id}\u{0}{key}")
    }

    /// [`plan_start`] plus the replay and the rate limit. A replay costs no start; a refused
    /// request costs none either. After running a `Fresh` plan, call [`StartGuards::remember`].
    pub fn plan(
        &self,
        device_id: &str,
        idempotency_key: Option<&str>,
        req: &StartRequest,
        ctx: &StartCtx,
    ) -> Result<Planned, StartError> {
        if ctx.relayed && !ctx.remote_start_enabled {
            return Err(StartError::REMOTE_START_OFF);
        }
        let now = self.now_ms();
        let ttl = IDEMPOTENCY_TTL.as_millis() as u64;
        if let Some(key) = idempotency_key.filter(|k| !k.is_empty()) {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.replies
                .retain(|_, (at, _)| now.saturating_sub(*at) < ttl);
            if let Some((_, reply)) = st.replies.get(&Self::reply_key(device_id, key)) {
                return Ok(Planned::Replay(reply.clone()));
            }
        }
        let plan = plan_start(req, ctx)?;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (credit, at) = st
            .buckets
            .get(device_id)
            .copied()
            .unwrap_or((MINUTE_MS, now));
        let credit = (credit + now.saturating_sub(at)).min(MINUTE_MS);
        if credit < START_COST_MS {
            st.buckets.insert(device_id.to_string(), (credit, now));
            return Err(StartError::RATE_LIMITED);
        }
        st.buckets
            .insert(device_id.to_string(), (credit - START_COST_MS, now));
        Ok(Planned::Fresh(plan))
    }

    /// Keeps the `201` body of a started session so a retry with the same key replays it.
    pub fn remember(&self, device_id: &str, idempotency_key: &str, response: &Value) {
        if idempotency_key.is_empty() {
            return;
        }
        let now = self.now_ms();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.replies.insert(
            Self::reply_key(device_id, idempotency_key),
            (now, response.clone()),
        );
    }
}

// ───────────────────────────────────────────────────────────────── tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const OPTIONS: &str =
        include_str!("../../../docs/contracts/session-start/options.response.json");
    const REQ_PATH: &str =
        include_str!("../../../docs/contracts/session-start/start.request.path.json");
    const REQ_GIT: &str =
        include_str!("../../../docs/contracts/session-start/start.request.git.json");
    const RES_ENGINE: &str =
        include_str!("../../../docs/contracts/session-start/start.response.engine.json");
    const RES_TERMINAL: &str =
        include_str!("../../../docs/contracts/session-start/start.response.terminal.json");
    const ERRORS: &str = include_str!("../../../docs/contracts/session-start/errors.json");

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "allternit-start-{}-{}",
                std::process::id(),
                name
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(std::fs::canonicalize(&p).unwrap())
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct World {
        installed: HashSet<String>,
        signed_out: HashSet<String>,
        modes: BTreeMap<String, RuntimeModes>,
        folders: Vec<StartFolder>,
    }

    impl World {
        fn new() -> Self {
            let installed = ["allternit", "claude", "codex", "kimi"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let mut modes = BTreeMap::new();
            modes.insert(
                "allternit".into(),
                RuntimeModes {
                    default: Mode::Default,
                    supported: Mode::ALL.to_vec(),
                },
            );
            modes.insert(
                "claude".into(),
                RuntimeModes {
                    default: Mode::Default,
                    supported: Mode::ALL.to_vec(),
                },
            );
            modes.insert(
                "codex".into(),
                RuntimeModes {
                    default: Mode::Auto,
                    supported: vec![Mode::Auto, Mode::Default, Mode::Bypass],
                },
            );
            World {
                installed,
                signed_out: HashSet::new(),
                modes,
                folders: vec![],
            }
        }
        fn ctx(&self) -> StartCtx<'_> {
            StartCtx {
                remote_start_enabled: true,
                relayed: true,
                caller_mode: None,
                installed: &self.installed,
                signed_out: &self.signed_out,
                modes: &self.modes,
                folders: &self.folders,
                git_resolver: None,
            }
        }
    }

    fn request(dir: &Path, runtime: &str, mode: Option<Mode>) -> StartRequest {
        StartRequest {
            folder: FolderRef::Path {
                path: dir.to_string_lossy().into_owned(),
            },
            runtime: runtime.into(),
            mode,
            confirm_bypass: false,
            prompt: Some("fix it".into()),
            origin: StartOrigin {
                surface: "ios".into(),
                device_name: "Phone".into(),
                parent_session_id: None,
                depth: Some(0),
            },
        }
    }

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        k.sort();
        k
    }

    #[test]
    fn start_fixtures_round_trip() {
        for raw in [REQ_PATH, REQ_GIT] {
            let fixture: Value = serde_json::from_str(raw).unwrap();
            let req: StartRequest = serde_json::from_str(raw).unwrap();
            assert_eq!(serde_json::to_value(&req).unwrap(), fixture);
        }
        let git: StartRequest = serde_json::from_str(REQ_GIT).unwrap();
        assert!(
            matches!(git.folder, FolderRef::Git { ref git } if git.branch == "main" && git.head.is_none())
        );
        for raw in [RES_ENGINE, RES_TERMINAL, OPTIONS] {
            let fixture: Value = serde_json::from_str(raw).unwrap();
            let back = if raw == OPTIONS {
                serde_json::to_value(serde_json::from_str::<StartOptions>(raw).unwrap()).unwrap()
            } else {
                serde_json::to_value(serde_json::from_str::<StartResponse>(raw).unwrap()).unwrap()
            };
            assert_eq!(back, fixture);
        }
        let errors: Vec<Value> = serde_json::from_str(ERRORS).unwrap();
        let ours: Vec<Value> = StartError::ALL
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect();
        assert_eq!(ours, errors);
        assert_eq!(
            keys(&StartError::RATE_LIMITED.body()),
            vec!["action", "error", "message"]
        );
    }

    #[test]
    fn start_options_shape_matches_contract() {
        let home = TempDir::new("home");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(home.path().join(".codex/config.toml"), "model = \"o3\"\nsandbox_mode = \"read-only\" # ask\n[profiles.x]\napproval_policy = \"never\"\n").unwrap();
        let older = home.path().join("older");
        let newer = home.path().join("newer");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::create_dir_all(&newer).unwrap();
        let t = |s| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        let options = build_options(OptionsInput {
            device: StartDevice {
                runtime_id: "rt_1".into(),
                name: "Mac".into(),
                os: "macos".into(),
                remote_start: true,
            },
            candidates: vec![
                FolderCandidate {
                    path: older.clone(),
                    last_used: t(1_000),
                },
                FolderCandidate {
                    path: newer.clone(),
                    last_used: t(2_000),
                },
                FolderCandidate {
                    path: older.clone(),
                    last_used: t(500),
                },
                FolderCandidate {
                    path: home.path().join("gone"),
                    last_used: t(3_000),
                },
                FolderCandidate {
                    path: "relative".into(),
                    last_used: t(3_000),
                },
            ],
            runtimes: vec!["claude".into(), "codex".into()],
            home: home.path().to_path_buf(),
        });
        assert_eq!(
            options
                .folders
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["newer", "older"]
        );
        assert_eq!(options.folders[1].last_used_at, "1970-01-01T00:16:40Z");
        assert_eq!(
            options.modes["claude"],
            RuntimeModes {
                default: Mode::Bypass,
                supported: Mode::ALL.to_vec()
            }
        );
        assert_eq!(
            options.modes["codex"],
            RuntimeModes {
                default: Mode::Default,
                supported: vec![Mode::Auto, Mode::Default, Mode::Bypass]
            }
        );
        assert_eq!(options.modes["allternit"].supported, Mode::ALL.to_vec());

        let ours = serde_json::to_value(&options).unwrap();
        let fixture: Value = serde_json::from_str(OPTIONS).unwrap();
        assert_eq!(keys(&ours), keys(&fixture));
        assert_eq!(keys(&ours["device"]), keys(&fixture["device"]));
        assert_eq!(keys(&ours["folders"][0]), keys(&fixture["folders"][3]));
        assert_eq!(
            keys(&ours["modes"]["claude"]),
            keys(&fixture["modes"]["claude"])
        );
        assert_eq!(ours["defaultRuntime"], "allternit");
    }

    #[test]
    fn start_options_read_git_facts() {
        let repo = TempDir::new("repo");
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        assert_eq!(git_info(repo.path()), None, "no commit yet");
        run(&[
            "remote",
            "add",
            "origin",
            "https://bot:ghp_secret@github.com/example-org/x.git",
        ]);
        run(&["commit", "-q", "--allow-empty", "-m", "first"]);
        let clean = git_info(repo.path()).unwrap();
        assert_eq!(
            clean.remote.as_deref(),
            Some("https://github.com/example-org/x.git")
        );
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert_eq!(clean.head.len(), 40);
        assert!(!clean.dirty);
        assert_eq!(clean.ahead, 0);
        std::fs::write(repo.path().join("new.txt"), "x").unwrap();
        assert!(git_info(repo.path()).unwrap().dirty);
        let plain = TempDir::new("plain");
        assert_eq!(git_info(plain.path()), None);

        assert_eq!(
            parse_status("# branch.oid abc\n# branch.head (detached)\n# branch.ab +3 -1\n")
                .unwrap()
                .ahead,
            3
        );
        assert_eq!(
            strip_credentials("git@github.com:o/x.git"),
            "git@github.com:o/x.git"
        );
        assert_eq!(
            remote_key("git@github.com:Example-Org/x.git"),
            remote_key("https://github.com/example-org/x")
        );
    }

    #[test]
    fn start_plans_engine_and_terminal() {
        let dir = TempDir::new("plans");
        let w = World::new();
        let ctx = w.ctx();
        match plan_start(&request(dir.path(), "allternit", None), &ctx).unwrap() {
            StartPlan::Engine {
                directory,
                mode,
                prompt,
                ..
            } => {
                assert_eq!(
                    (directory.as_path(), mode, prompt.as_deref()),
                    (dir.path(), Mode::Default, Some("fix it"))
                );
            }
            other => panic!("{other:?}"),
        }
        match plan_start(&request(dir.path(), "claude", Some(Mode::Plan)), &ctx).unwrap() {
            StartPlan::Terminal {
                argv,
                cwd,
                agent,
                send_after_start,
                title,
            } => {
                assert_eq!(argv, vec!["claude", "--permission-mode", "plan", "fix it"]);
                assert_eq!(
                    (cwd.as_path(), agent.as_str(), send_after_start),
                    (dir.path(), "claude", None)
                );
                assert!(title.starts_with("claude · allternit-start-"));
            }
            other => panic!("{other:?}"),
        }
        // No mode asked: no flags (the CLI's own default). A CLI without a prompt argument gets it typed in.
        match plan_start(&request(dir.path(), "kimi", None), &ctx).unwrap() {
            StartPlan::Terminal {
                argv,
                send_after_start,
                ..
            } => {
                assert_eq!(argv, vec!["kimi"]);
                assert_eq!(send_after_start.as_deref(), Some("fix it"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn start_guards_refuse() {
        let dir = TempDir::new("guards");
        let mut w = World::new();
        let err = |w: &World, req: &StartRequest, f: &dyn Fn(&mut StartCtx)| {
            let mut ctx = w.ctx();
            f(&mut ctx);
            plan_start(req, &ctx).err()
        };
        let base = request(dir.path(), "claude", Some(Mode::Default));

        // Safety switch: off refuses relayed starts only.
        assert_eq!(
            err(&w, &base, &|c| c.remote_start_enabled = false),
            Some(StartError::REMOTE_START_OFF)
        );
        assert_eq!(
            err(&w, &base, &|c| {
                c.remote_start_enabled = false;
                c.relayed = false
            }),
            None
        );

        // Mode never above the caller's (plan < default < auto < bypass).
        let auto = request(dir.path(), "claude", Some(Mode::Auto));
        assert_eq!(
            err(&w, &auto, &|c| c.caller_mode = Some(Mode::Default)),
            Some(StartError::MODE_NOT_ALLOWED)
        );
        assert_eq!(
            err(&w, &base, &|c| c.caller_mode = Some(Mode::Default)),
            None
        );
        assert_eq!(
            err(&w, &request(dir.path(), "claude", Some(Mode::Plan)), &|c| {
                c.caller_mode = Some(Mode::Plan)
            }),
            None
        );
        // An unasked mode is the device default, and is capped too.
        assert_eq!(
            err(&w, &request(dir.path(), "codex", None), &|c| c
                .caller_mode =
                Some(Mode::Default)),
            Some(StartError::MODE_NOT_ALLOWED)
        );
        // A mode the CLI has no flag for.
        assert_eq!(
            err(&w, &request(dir.path(), "codex", Some(Mode::Plan)), &|_| {}),
            Some(StartError::MODE_NOT_ALLOWED)
        );

        // Bypass from a relayed request: only with the device default bypass, or a confirm.
        let mut bypass = request(dir.path(), "claude", Some(Mode::Bypass));
        assert_eq!(
            err(&w, &bypass, &|_| {}),
            Some(StartError::MODE_NOT_ALLOWED)
        );
        assert_eq!(err(&w, &bypass, &|c| c.relayed = false), None);
        bypass.confirm_bypass = true;
        assert_eq!(err(&w, &bypass, &|_| {}), None);
        bypass.confirm_bypass = false;
        w.modes.get_mut("claude").unwrap().default = Mode::Bypass;
        assert_eq!(err(&w, &bypass, &|_| {}), None);
        w.modes.get_mut("claude").unwrap().default = Mode::Default;

        // Depth: three levels at most.
        let mut deep = base.clone();
        deep.origin.depth = Some(3);
        assert_eq!(err(&w, &deep, &|_| {}), None);
        deep.origin.depth = Some(4);
        assert_eq!(err(&w, &deep, &|_| {}), Some(StartError::DEPTH_EXCEEDED));

        // Folder and runtime.
        let gone = request(&dir.path().join("gone"), "claude", None);
        assert_eq!(err(&w, &gone, &|_| {}), Some(StartError::FOLDER_MISSING));
        let mut relative = base.clone();
        relative.folder = FolderRef::Path {
            path: "code/x".into(),
        };
        assert_eq!(
            err(&w, &relative, &|_| {}),
            Some(StartError::FOLDER_MISSING)
        );
        assert_eq!(
            err(&w, &request(dir.path(), "grok", None), &|_| {}),
            Some(StartError::RUNTIME_MISSING)
        );
        w.installed.remove("codex");
        assert_eq!(
            err(&w, &request(dir.path(), "codex", None), &|_| {}),
            Some(StartError::RUNTIME_MISSING)
        );
        w.signed_out.insert("claude".into());
        assert_eq!(
            err(&w, &base, &|_| {}),
            Some(StartError::RUNTIME_SIGNED_OUT)
        );
    }

    #[test]
    fn start_git_folder_resolves() {
        let dir = TempDir::new("gitref");
        let mut w = World::new();
        w.folders = vec![StartFolder {
            path: dir.path().to_string_lossy().into_owned(),
            name: "x".into(),
            last_used_at: "2026-11-27T15:42:10Z".into(),
            git: Some(GitInfo {
                remote: Some("git@github.com:example-org/x.git".into()),
                branch: Some("main".into()),
                head: "a".repeat(40),
                dirty: false,
                ahead: 0,
            }),
        }];
        let mut req = request(dir.path(), "allternit", None);
        req.folder = FolderRef::Git {
            git: GitRef {
                remote: "https://github.com/example-org/x.git".into(),
                branch: "main".into(),
                head: None,
            },
        };
        assert!(
            matches!(plan_start(&req, &w.ctx()), Ok(StartPlan::Engine { ref directory, .. }) if directory == dir.path())
        );
        req.folder = FolderRef::Git {
            git: GitRef {
                remote: "https://github.com/example-org/x.git".into(),
                branch: "dev".into(),
                head: None,
            },
        };
        assert_eq!(
            plan_start(&req, &w.ctx()).err(),
            Some(StartError::FOLDER_MISSING)
        );
        // A cloud computer's resolver clones; its failure is the start's error.
        let clone_fails =
            |_: &GitRef| -> Result<PathBuf, StartError> { Err(StartError::CLONE_FAILED) };
        let mut ctx = w.ctx();
        ctx.git_resolver = Some(&clone_fails);
        assert_eq!(plan_start(&req, &ctx).err(), Some(StartError::CLONE_FAILED));
    }

    fn fake_clock() -> (Arc<AtomicU64>, Clock) {
        let ms = Arc::new(AtomicU64::new(0));
        let read = ms.clone();
        (
            ms,
            Arc::new(move || Duration::from_millis(read.load(Ordering::SeqCst))),
        )
    }

    #[test]
    fn start_rate_limit_six_a_minute() {
        let dir = TempDir::new("rate");
        let w = World::new();
        let (ms, clock) = fake_clock();
        let guards = StartGuards::with_clock(clock);
        let req = request(dir.path(), "allternit", None);
        // A refused request costs nothing.
        let gone = request(&dir.path().join("gone"), "allternit", None);
        for _ in 0..3 {
            assert_eq!(
                guards.plan("rt_1", None, &gone, &w.ctx()).err(),
                Some(StartError::FOLDER_MISSING)
            );
        }
        for i in 0..6 {
            assert!(
                matches!(
                    guards.plan("rt_1", None, &req, &w.ctx()),
                    Ok(Planned::Fresh(_))
                ),
                "start {i}"
            );
        }
        assert_eq!(
            guards.plan("rt_1", None, &req, &w.ctx()).err(),
            Some(StartError::RATE_LIMITED)
        );
        // Another device has its own bucket.
        assert!(guards.plan("rt_2", None, &req, &w.ctx()).is_ok());
        ms.store(9_999, Ordering::SeqCst);
        assert_eq!(
            guards.plan("rt_1", None, &req, &w.ctx()).err(),
            Some(StartError::RATE_LIMITED)
        );
        ms.store(10_000, Ordering::SeqCst);
        assert!(guards.plan("rt_1", None, &req, &w.ctx()).is_ok());
        assert_eq!(
            guards.plan("rt_1", None, &req, &w.ctx()).err(),
            Some(StartError::RATE_LIMITED)
        );
        ms.store(10_000 + 120_000, Ordering::SeqCst);
        for _ in 0..6 {
            assert!(guards.plan("rt_1", None, &req, &w.ctx()).is_ok());
        }
        assert_eq!(
            guards.plan("rt_1", None, &req, &w.ctx()).err(),
            Some(StartError::RATE_LIMITED)
        );
    }

    #[test]
    fn start_idempotency_replays_first_result() {
        let dir = TempDir::new("idem");
        let w = World::new();
        let (ms, clock) = fake_clock();
        let guards = StartGuards::with_clock(clock);
        let req = request(dir.path(), "allternit", None);
        assert!(matches!(
            guards.plan("rt_1", Some("k1"), &req, &w.ctx()),
            Ok(Planned::Fresh(_))
        ));
        let first: Value = serde_json::from_str(RES_ENGINE).unwrap();
        guards.remember("rt_1", "k1", &first);
        // Replays cost no start: ten retries, then five fresh starts still fit.
        for _ in 0..10 {
            assert_eq!(
                guards.plan("rt_1", Some("k1"), &req, &w.ctx()),
                Ok(Planned::Replay(first.clone()))
            );
        }
        for _ in 0..5 {
            assert!(matches!(
                guards.plan("rt_1", Some("other"), &req, &w.ctx()),
                Ok(Planned::Fresh(_))
            ));
        }
        // Keys are per device, and the switch still applies to a replay.
        ms.store(60_000, Ordering::SeqCst);
        assert!(matches!(
            guards.plan("rt_2", Some("k1"), &req, &w.ctx()),
            Ok(Planned::Fresh(_))
        ));
        let mut off = w.ctx();
        off.remote_start_enabled = false;
        assert_eq!(
            guards.plan("rt_1", Some("k1"), &req, &off).err(),
            Some(StartError::REMOTE_START_OFF)
        );
        // After ten minutes the key is new again.
        ms.store(600_000 - 1, Ordering::SeqCst);
        assert!(matches!(
            guards.plan("rt_1", Some("k1"), &req, &w.ctx()),
            Ok(Planned::Replay(_))
        ));
        ms.store(600_000, Ordering::SeqCst);
        assert!(matches!(
            guards.plan("rt_1", Some("k1"), &req, &w.ctx()),
            Ok(Planned::Fresh(_))
        ));
    }
}
