//! The harness table: the one place per-CLI detail lives for live sessions
//! (design decision 10 of the terminal workspace plan). Clients never learn
//! harness names; they read `LiveSession.movable`, `carriedFlags` and
//! `canCopy`, which come from here.
//!
//! Per harness: the process names a scan matches, the command that resumes a
//! session in another terminal (Move), the command that forks it (Copy), the
//! flag rules that decide which of the holder's own flags carry over, and how
//! a live process maps to its session id.
//!
//! Flags were read from each CLI's `--help` (2026-11-20/21, S0 and S1 checks):
//!
//! | harness  | Move                       | Copy                    |
//! |----------|----------------------------|-------------------------|
//! | claude   | `claude --resume <id>`     | `+ --fork-session`      |
//! | codex    | `codex resume <id>`        | `codex fork <id>`       |
//! | kimi     | `kimi --session <id>`      | `kimi fork <id>`        |
//! | grok     | `grok --resume <id>`       | `+ --fork-session`      |
//! | agy      | `agy --conversation <id>`  | none                    |
//! | gizzi    | `gizzi --session <id>`     | `+ --fork`              |
//! | opencode | `opencode --session <id>`  | `+ --fork`              |
//! | qwen     | `qwen --resume <id>`       | `+ --fork-session`      |
//!
//! Carried flags go right after the command name (`claude
//! --dangerously-skip-permissions --resume <id>`), in the order the holder
//! was started with them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// One harness row. Templates use `{flags}` (the carried flags) and `{id}`.
#[derive(Debug)]
pub struct Harness {
    pub id: &'static str,
    /// Process names (`argv[0]` or `comm` basename) a scan matches.
    pub procs: &'static [&'static str],
    move_argv: Option<&'static [&'static str]>,
    copy_argv: Option<&'static [&'static str]>,
    /// Dropped with their value (the value is taken only when it does not start with `-`).
    strip_value: &'static [&'static str],
    /// Dropped alone.
    strip_bare: &'static [&'static str],
    /// Leading subcommands dropped with the id after them (`resume <id>`).
    strip_subcommands: &'static [&'static str],
    /// A process whose first positional is one of these is not an interactive session.
    skip_subcommands: &'static [&'static str],
    /// The only positional the interactive CLI takes is a folder; any other first positional is a subcommand.
    positional_is_path: bool,
    /// Kept alone: a bare word after them is prompt text, not their value.
    bool_flags: &'static [&'static str],
    /// Kept with their value even when the value has spaces (a prompt-like value after an unknown flag is dropped).
    value_flags: &'static [&'static str],
    /// Kept with every bare word after them (variadic lists).
    multi_flags: &'static [&'static str],
    /// Flags whose value is the id of the session this process runs.
    id_flags: &'static [&'static str],
    /// Subcommands whose next word is the id of the session this process runs.
    id_subcommands: &'static [&'static str],
}

const HELP: [&str; 6] = ["-h", "--help", "-v", "-V", "--version", "--"];

pub static HARNESSES: &[Harness] = &[
    Harness {
        id: "claude",
        procs: &["claude"],
        move_argv: Some(&["claude", "{flags}", "--resume", "{id}"]),
        copy_argv: Some(&["claude", "{flags}", "--resume", "{id}", "--fork-session"]),
        strip_value: &["-r", "--resume", "--session-id", "--from-pr", "--teleport", "--cloud", "-w", "--worktree", "--environment"],
        strip_bare: &["-c", "--continue", "-p", "--print", "--fork-session", "--bg", "--background", "--desktop", "--tmux", "--no-session-persistence"],
        strip_subcommands: &[],
        skip_subcommands: &["mcp", "config", "doctor", "update", "upgrade", "install", "setup-token", "plugin", "migrate-installer", "auth"],
        positional_is_path: false,
        bool_flags: &[
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
            "--ax-screen-reader",
            "--bare",
            "--brief",
            "--chrome",
            "--no-chrome",
            "--disable-slash-commands",
            "--exclude-dynamic-system-prompt-sections",
            "--forward-subagent-text",
            "--ide",
            "--include-hook-events",
            "--include-partial-messages",
            "--replay-user-messages",
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--verbose",
        ],
        value_flags: &["--append-system-prompt", "--system-prompt", "--agents", "--settings", "--json-schema", "-n", "--name"],
        multi_flags: &["--add-dir", "--allowedTools", "--allowed-tools", "--disallowedTools", "--disallowed-tools", "--betas", "--mcp-config", "--file", "--tools"],
        id_flags: &["-r", "--resume", "--session-id"],
        id_subcommands: &[],
    },
    Harness {
        id: "codex",
        procs: &["codex"],
        move_argv: Some(&["codex", "{flags}", "resume", "{id}"]),
        copy_argv: Some(&["codex", "{flags}", "fork", "{id}"]),
        // `codex -c` is a config override and `-p` a profile: both carry.
        strip_value: &[],
        strip_bare: &["--last", "--all", "--worktree"],
        strip_subcommands: &["resume", "fork"],
        skip_subcommands: &[
            "exec", "e", "app-server", "mcp", "mcp-server", "login", "logout", "completion", "debug", "apply", "a", "sandbox", "cloud", "features", "daemon",
            "review", "help",
        ],
        positional_is_path: false,
        bool_flags: &[
            "--strict-config",
            "--oss",
            "--approve-for-me",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
            "--search",
            "--no-alt-screen",
            "--no-daemon",
            "--yolo",
            "--full-auto",
        ],
        value_flags: &["-c", "--config"],
        multi_flags: &["-i", "--image"],
        id_flags: &[],
        id_subcommands: &["resume"],
    },
    Harness {
        id: "kimi",
        procs: &["kimi-code", "kimi"],
        move_argv: Some(&["kimi", "{flags}", "--session", "{id}"]),
        copy_argv: Some(&["kimi", "{flags}", "fork", "{id}"]),
        strip_value: &["-S", "--session", "-r", "--resume", "-p", "--prompt"],
        strip_bare: &["-c", "--continue"],
        strip_subcommands: &["fork"],
        skip_subcommands: &["login", "logout", "mcp", "acp", "info", "upgrade", "update", "help"],
        positional_is_path: false,
        bool_flags: &["-y", "--yolo", "--auto", "--plan"],
        value_flags: &[],
        multi_flags: &[],
        id_flags: &["-S", "--session", "-r", "--resume"],
        id_subcommands: &[],
    },
    Harness {
        id: "grok",
        procs: &["grok"],
        move_argv: Some(&["grok", "{flags}", "--resume", "{id}"]),
        copy_argv: Some(&["grok", "{flags}", "--resume", "{id}", "--fork-session"]),
        strip_value: &["-r", "--resume", "-s", "--session-id", "-p", "--single", "-w", "--worktree", "--worktree-ref", "--prompt-file", "--prompt-json"],
        strip_bare: &["-c", "--continue", "--fork-session"],
        strip_subcommands: &[],
        skip_subcommands: &["login", "logout", "mcp", "update", "help"],
        positional_is_path: false,
        bool_flags: &[
            "--always-approve",
            "--debug",
            "--disable-web-search",
            "--fullscreen",
            "--include-partial-messages",
            "--memory-flush",
            "--minimal",
            "--no-alt-screen",
            "--no-memory",
            "--no-plan",
            "--no-subagents",
            "--oauth",
            "--restore-code",
            "--verbatim",
        ],
        value_flags: &["--system-prompt-override", "--agents", "--rules", "--json-schema"],
        multi_flags: &[],
        id_flags: &["-r", "--resume", "-s", "--session-id"],
        id_subcommands: &[],
    },
    Harness {
        id: "agy",
        procs: &["agy"],
        move_argv: Some(&["agy", "{flags}", "--conversation", "{id}"]),
        copy_argv: None,
        strip_value: &["--conversation", "--print", "--prompt", "--prompt-interactive", "--print-timeout"],
        strip_bare: &["--continue", "--new-project"],
        strip_subcommands: &[],
        skip_subcommands: &["login", "logout", "mcp", "update", "help"],
        positional_is_path: false,
        bool_flags: &["--dangerously-skip-permissions", "--disable-slash-commands", "--remote-agent", "--remote-control", "--sandbox"],
        value_flags: &["--settings", "--json-schema"],
        multi_flags: &[],
        id_flags: &["--conversation"],
        id_subcommands: &[],
    },
    Harness {
        id: "gizzi",
        procs: &["gizzi-code", "gizzi"],
        move_argv: Some(&["gizzi", "{flags}", "--session", "{id}"]),
        copy_argv: Some(&["gizzi", "{flags}", "--session", "{id}", "--fork"]),
        strip_value: &["-s", "--session", "--prompt"],
        strip_bare: &["-c", "--continue", "--fork", "--worktree", "--no-worktree"],
        strip_subcommands: &[],
        skip_subcommands: &[],
        positional_is_path: true,
        bool_flags: &["--yolo", "--dangerously-skip-permissions", "--dangerously-skip-sandbox", "--mdns"],
        value_flags: &[],
        multi_flags: &[],
        id_flags: &["-s", "--session"],
        id_subcommands: &[],
    },
    Harness {
        id: "opencode",
        procs: &["opencode"],
        move_argv: Some(&["opencode", "{flags}", "--session", "{id}"]),
        copy_argv: Some(&["opencode", "{flags}", "--session", "{id}", "--fork"]),
        strip_value: &["-s", "--session", "--prompt"],
        strip_bare: &["-c", "--continue", "--fork"],
        strip_subcommands: &[],
        skip_subcommands: &[],
        positional_is_path: true,
        bool_flags: &["--print-logs", "--pure", "--mdns", "--auto", "--mini", "--no-replay"],
        value_flags: &[],
        multi_flags: &[],
        id_flags: &["-s", "--session"],
        id_subcommands: &[],
    },
    // `qwen --help` (installed CLI, 11-21): `-r/--resume <id>` and `--fork-session`. The id comes
    // from argv only; there is no store lookup for qwen, so a qwen started without one is no_resume.
    Harness {
        id: "qwen",
        procs: &["qwen"],
        move_argv: Some(&["qwen", "{flags}", "--resume", "{id}"]),
        copy_argv: Some(&["qwen", "{flags}", "--resume", "{id}", "--fork-session"]),
        strip_value: &["-r", "--resume", "--session-id", "-p", "--prompt", "-i", "--prompt-interactive"],
        strip_bare: &["-c", "--continue", "--fork-session", "--worktree"],
        strip_subcommands: &[],
        skip_subcommands: &["mcp", "extensions", "help"],
        positional_is_path: false,
        bool_flags: &[
            "-d",
            "--debug",
            "--bare",
            "--safe-mode",
            "--insecure",
            "--chat-recording",
            "-y",
            "--yolo",
            "--acp",
            "--experimental-lsp",
            "--screen-reader",
            "--include-partial-messages",
            "--openai-logging",
            "--telemetry",
            "--telemetry-log-prompts",
        ],
        value_flags: &["--system-prompt", "--append-system-prompt", "--proxy"],
        multi_flags: &["--allowed-mcp-server-names", "--disable-mcp-server", "--allowed-tools", "-e", "--extensions", "--include-directories", "--add-dir"],
        id_flags: &["-r", "--resume", "--session-id"],
        id_subcommands: &[],
    },
];

/// Interpreters a script CLI runs under (`node /opt/homebrew/bin/qwen …`).
pub(crate) const INTERPRETERS: [&str; 6] = ["node", "bun", "deno", "python", "python3", "Python"];

pub fn harness(id: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|h| h.id == id)
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// `argv` from the command name on: a leading interpreter and its own flags are dropped.
fn from_bin(argv: &[String]) -> &[String] {
    let Some(first) = argv.first() else { return argv };
    if !INTERPRETERS.contains(&basename(first)) {
        return argv;
    }
    let script = argv.iter().skip(1).position(|a| !a.starts_with('-')).map_or(argv.len(), |i| i + 1);
    &argv[script.min(argv.len())..]
}

/// The harness a process runs, from its argv (or its `comm` when argv is empty), with argv from the command name on.
pub fn harness_for_process(comm: &str, argv: &[String]) -> Option<(&'static Harness, Vec<String>)> {
    let argv = from_bin(argv);
    let name = argv.first().map(|a| basename(a)).unwrap_or_else(|| basename(comm));
    let h = HARNESSES.iter().find(|h| h.procs.contains(&name))?;
    Some((h, argv.to_vec()))
}

fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with('.') || s.starts_with('~')
}

/// Free text (a prompt), not a flag value: it has whitespace and is not a path.
fn looks_like_prompt(s: &str) -> bool {
    s.chars().any(char::is_whitespace) && !looks_like_path(s)
}

/// `--name=value` → (`--name`, Some(value)); `-x` → (`-x`, None).
fn split_flag(a: &str) -> (&str, Option<&str>) {
    match a.split_once('=') {
        Some((n, v)) if n.starts_with('-') => (n, Some(v)),
        _ => (a, None),
    }
}

fn is_flag(a: &str) -> bool {
    a.len() > 1 && a.starts_with('-')
}

/// Whether this argv is an interactive session of `h` (not a subcommand like `codex exec`).
pub fn is_interactive(h: &Harness, argv: &[String]) -> bool {
    let first_positional = positionals(h, argv).into_iter().next();
    match first_positional {
        None => true,
        Some(p) if h.strip_subcommands.contains(&p.as_str()) || h.id_subcommands.contains(&p.as_str()) => true,
        Some(p) if h.skip_subcommands.contains(&p.as_str()) => false,
        Some(p) if h.positional_is_path => looks_like_path(&p),
        Some(_) => true,
    }
}

/// The bare words of argv (after the command name), skipping flag values the table knows about.
fn positionals(h: &Harness, argv: &[String]) -> Vec<String> {
    let args = argv.get(1..).unwrap_or_default();
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            out.extend(args[i + 1..].iter().cloned());
            break;
        }
        if is_flag(a) {
            let (name, inline) = split_flag(a);
            let takes_value = !h.bool_flags.contains(&name) && !h.strip_bare.contains(&name) && !HELP.contains(&name);
            i += 1;
            if inline.is_none() && takes_value && args.get(i).is_some_and(|n| !is_flag(n) && !looks_like_prompt(n)) {
                i += 1;
            }
            continue;
        }
        out.push(a.to_string());
        i += 1;
    }
    out
}

/// The holder's own flags that a Move or Copy keeps, in order: everything except the
/// command name, resume/continue/session/conversation flags and their ids, one-shot
/// print/prompt flags, fork flags, and positional prompt text.
pub fn carried_flags(harness: &str, argv: &[String]) -> Vec<String> {
    let Some(h) = self::harness(harness) else { return Vec::new() };
    let argv = from_bin(argv);
    let args = argv.get(1..).unwrap_or_default();
    let mut out = Vec::new();
    let mut seen_positional = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        if is_flag(a) {
            let (name, inline) = split_flag(a);
            i += 1;
            let next_bare = |i: usize| args.get(i).is_some_and(|n| !is_flag(n));
            if h.strip_value.contains(&name) {
                if inline.is_none() && next_bare(i) {
                    i += 1;
                }
                continue;
            }
            if h.strip_bare.contains(&name) || name.starts_with("--fork") || HELP.contains(&name) {
                continue;
            }
            out.push(a.to_string());
            if inline.is_some() || h.bool_flags.contains(&name) {
                continue;
            }
            if h.multi_flags.contains(&name) {
                while next_bare(i) {
                    out.push(args[i].clone());
                    i += 1;
                }
            } else if next_bare(i) && (h.value_flags.contains(&name) || !looks_like_prompt(&args[i])) {
                out.push(args[i].clone());
                i += 1;
            }
            continue;
        }
        // A bare word: a resume/fork subcommand (with its id) or prompt/project text. Neither carries.
        if !seen_positional && h.strip_subcommands.contains(&a) {
            i += 1;
            if args.get(i).is_some_and(|n| !is_flag(n)) {
                i += 1;
            }
        } else {
            i += 1;
        }
        seen_positional = true;
    }
    out
}

/// The id of the session this argv runs, when it names one (`--session X`, `resume X`,
/// `--conversation=X`). A fork (`--fork*`, `fork X`) runs a new id, so it names none.
pub fn argv_session_id(harness: &str, argv: &[String]) -> Option<String> {
    let h = self::harness(harness)?;
    let argv = from_bin(argv);
    let args = argv.get(1..).unwrap_or_default();
    if args.iter().any(|a| a.starts_with("--fork") || a == "fork") {
        return None;
    }
    let mut i = 0;
    let mut seen_positional = false;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        if is_flag(a) {
            let (name, inline) = split_flag(a);
            if h.id_flags.contains(&name) {
                let v = inline.map(str::to_string).or_else(|| args.get(i + 1).filter(|n| !is_flag(n)).cloned());
                return v.filter(|v| valid_id(v));
            }
            // Skip this flag's value, as `positionals` does, so `codex -c k=v resume <id>` still finds `resume`.
            let takes_value = !h.bool_flags.contains(&name) && !h.strip_bare.contains(&name) && !HELP.contains(&name);
            i += 1;
            if inline.is_none() && takes_value && args.get(i).is_some_and(|n| !is_flag(n) && !looks_like_prompt(n) && !h.id_subcommands.contains(&n.as_str())) {
                i += 1;
            }
            continue;
        }
        if !seen_positional && h.id_subcommands.contains(&a) {
            return args.get(i + 1).filter(|n| !is_flag(n)).cloned().filter(|v| valid_id(v));
        }
        seen_positional = true;
        i += 1;
    }
    None
}

/// A session id safe to put in an argv (and in a shell line): no spaces, quotes or leading dash.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

fn expand(template: &[&str], session_id: &str, carried: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(template.len() + carried.len());
    for t in template {
        match *t {
            "{flags}" => out.extend(carried.iter().cloned()),
            "{id}" => out.push(session_id.to_string()),
            other => out.push(other.to_string()),
        }
    }
    out
}

/// The command that resumes `session_id` in another terminal, with the carried flags.
/// `None`: the harness cannot resume by id, or the id is missing or unsafe.
pub fn move_argv(harness: &str, session_id: &str, carried: &[String]) -> Option<Vec<String>> {
    let t = self::harness(harness)?.move_argv?;
    valid_id(session_id).then(|| expand(t, session_id, carried))
}

/// The command that starts a new session of this harness with the carried flags ("Open a new
/// session here"). `None`: not a known harness.
pub fn new_argv(harness: &str, carried: &[String]) -> Option<Vec<String>> {
    let h = self::harness(harness)?;
    let program = h.move_argv.or(h.copy_argv).and_then(|t| t.first().copied()).unwrap_or(h.id);
    Some(std::iter::once(program.to_string()).chain(carried.iter().cloned()).collect())
}

/// The command that forks `session_id` into a new session, with the carried flags.
pub fn copy_argv(harness: &str, session_id: &str, carried: &[String]) -> Option<Vec<String>> {
    let t = self::harness(harness)?.copy_argv?;
    valid_id(session_id).then(|| expand(t, session_id, carried))
}

// ---------------------------------------------------------------- session ids from the stores

/// A process's session id and when its store was last written (a recent write means it is mid-turn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub id: String,
    pub last_write: Option<SystemTime>,
}

fn home() -> PathBuf {
    crate::platform::user_home_or("/")
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The session a live harness process runs, cheapest reliable source first: the id in
/// its argv, then the harness's own store (kimi's lock registry, the file it holds open,
/// the newest session for its folder). Claude Code comes from its pid registry instead.
pub fn resolve_session(harness: &str, pid: u32, cwd: &str, argv: &[String]) -> Option<Resolved> {
    if let Some(id) = argv_session_id(harness, argv) {
        return Some(Resolved { last_write: last_write(harness, &id, cwd), id });
    }
    match harness {
        "kimi" => kimi_by_lock(pid),
        "codex" => codex_by_open_file(pid).or_else(|| codex_newest_for(cwd)),
        "gizzi" => gizzi_by_open_file(pid).or_else(|| gizzi_newest_for(cwd)),
        "agy" => agy_by_presence_lock(pid),
        "grok" => grok_active(pid).or_else(|| grok_newest_for(cwd)),
        "opencode" => opencode_newest_for(cwd),
        _ => None,
    }
}

/// When the store of a known session id was last written.
fn last_write(harness: &str, id: &str, cwd: &str) -> Option<SystemTime> {
    match harness {
        "codex" => codex_rollouts().into_iter().find(|(p, _)| rollout_id(p).as_deref() == Some(id)).map(|(_, t)| t),
        "kimi" => kimi_session_dir(id).and_then(|d| mtime(&d.join("state.json")).or_else(|| mtime(&d))),
        "gizzi" => gizzi_project_dirs(cwd).into_iter().find_map(|d| mtime(&d.join(format!("{id}.jsonl")))),
        "agy" => mtime(&agy_dir().join("brain").join(id)),
        "grok" => mtime(&grok_dir().join("sessions").join(&*urlencoding::encode(cwd)).join(id)),
        _ => None,
    }
}

fn newest_in(dir: &Path, keep: impl Fn(&Path) -> bool) -> Option<(PathBuf, SystemTime)> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| keep(p.as_path())).filter_map(|p| Some((mtime(&p)?, p))).max().map(|(t, p)| (p, t))
}

// kimi: `~/.kimi-code/locks/sessions/session_<uuid>.lock` holds `{pid, startedAt}`.
fn kimi_dir() -> PathBuf {
    home().join(".kimi-code")
}

fn kimi_by_lock(pid: u32) -> Option<Resolved> {
    let dir = std::fs::read_dir(kimi_dir().join("locks").join("sessions")).ok()?;
    let id = dir
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let id = p.file_name()?.to_str()?.strip_suffix(".lock")?.to_string();
            let v: Value = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            (v["pid"].as_u64() == Some(u64::from(pid))).then(|| (v["startedAt"].as_i64().unwrap_or(0), id))
        })
        .max()?
        .1;
    valid_id(&id).then(|| Resolved { last_write: last_write("kimi", &id, ""), id })
}

/// `~/.kimi-code/sessions/wd_<…>/<id>`.
fn kimi_session_dir(id: &str) -> Option<PathBuf> {
    std::fs::read_dir(kimi_dir().join("sessions")).ok()?.flatten().map(|e| e.path().join(id)).find(|p| p.is_dir())
}

// codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`; the first line is `session_meta` with `cwd`.
fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map_or_else(|| home().join(".codex"), PathBuf::from)
}

fn rollout_id(p: &Path) -> Option<String> {
    let stem = p.file_name()?.to_str()?.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    // The uuid is the last 36 characters.
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    (id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4).then(|| id.to_string())
}

/// Every rollout with its modified time, newest first.
fn codex_rollouts() -> Vec<(PathBuf, SystemTime)> {
    let mut v: Vec<(PathBuf, SystemTime)> = walkdir::WalkDir::new(codex_dir().join("sessions"))
        .max_depth(4)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let p = e.into_path();
            rollout_id(&p)?;
            let t = mtime(&p)?;
            Some((p, t))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// The `cwd` in a rollout's first line (cached: the first line never changes).
fn rollout_cwd(p: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = cache.get(p) {
        return v.clone();
    }
    use std::io::{BufRead, Read};
    let mut line = String::new();
    let read = std::fs::File::open(p).map(|f| std::io::BufReader::new(f.take(256 * 1024)).read_line(&mut line));
    let cwd = read.ok().and_then(|_| serde_json::from_str::<Value>(&line).ok()).and_then(|v| v["payload"]["cwd"].as_str().map(str::to_string));
    cache.insert(p.to_path_buf(), cwd.clone());
    cwd
}

fn codex_by_open_file(pid: u32) -> Option<Resolved> {
    super::proc_scan::open_paths(pid).iter().map(PathBuf::from).find_map(|p| {
        let id = rollout_id(&p)?;
        Some(Resolved { last_write: mtime(&p), id })
    })
}

fn codex_newest_for(cwd: &str) -> Option<Resolved> {
    if cwd.is_empty() {
        return None;
    }
    codex_rollouts().into_iter().take(200).find(|(p, _)| rollout_cwd(p).as_deref() == Some(cwd)).and_then(|(p, t)| Some(Resolved { id: rollout_id(&p)?, last_write: Some(t) }))
}

// gizzi: `~/.gizzi/projects/<cwd slug>/<uuid>.jsonl`; the slug is the folder with `/` turned into `-`.
fn gizzi_project_dirs(cwd: &str) -> Vec<PathBuf> {
    if cwd.is_empty() {
        return Vec::new();
    }
    let root = home().join(".gizzi").join("projects");
    let all: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let slash = cwd.replace('/', "-");
    let mut dirs = vec![root.join(all)];
    if dirs[0] != root.join(&slash) {
        dirs.push(root.join(slash));
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

fn gizzi_by_open_file(pid: u32) -> Option<Resolved> {
    let root = home().join(".gizzi").join("projects");
    super::proc_scan::open_paths(pid).iter().map(PathBuf::from).find_map(|p| {
        (p.starts_with(&root) && p.extension().is_some_and(|x| x == "jsonl")).then_some(())?;
        let id = p.file_stem()?.to_str()?.to_string();
        valid_id(&id).then(|| Resolved { last_write: mtime(&p), id })
    })
}

fn gizzi_newest_for(cwd: &str) -> Option<Resolved> {
    gizzi_project_dirs(cwd)
        .into_iter()
        .filter_map(|d| newest_in(&d, |p| p.extension().is_some_and(|x| x == "jsonl")))
        .max_by_key(|(_, t)| *t)
        .and_then(|(p, t)| Some(Resolved { id: p.file_stem()?.to_str()?.to_string(), last_write: Some(t) }))
        .filter(|r| valid_id(&r.id))
}

// agy: the live process holds `~/.gemini/antigravity-cli/presence/<uuid>.lock` open (the files are empty).
fn agy_dir() -> PathBuf {
    home().join(".gemini").join("antigravity-cli")
}

fn agy_by_presence_lock(pid: u32) -> Option<Resolved> {
    let presence = agy_dir().join("presence");
    super::proc_scan::open_paths(pid).iter().map(PathBuf::from).find_map(|p| {
        (p.parent() == Some(presence.as_path())).then_some(())?;
        let id = p.file_name()?.to_str()?.strip_suffix(".lock")?.to_string();
        valid_id(&id).then(|| Resolved { last_write: last_write("agy", &id, ""), id })
    })
}

// grok: `~/.grok/active_sessions.json` (entries with a pid while running), and
// `~/.grok/sessions/<url-encoded cwd>/<id>`.
fn grok_dir() -> PathBuf {
    home().join(".grok")
}

fn grok_active(pid: u32) -> Option<Resolved> {
    let v: Value = serde_json::from_slice(&std::fs::read(grok_dir().join("active_sessions.json")).ok()?).ok()?;
    let list = v.as_array().cloned().or_else(|| v["sessions"].as_array().cloned())?;
    let entry = list.iter().find(|e| e["pid"].as_u64() == Some(u64::from(pid)))?;
    let id = ["sessionId", "session_id", "id"].iter().find_map(|k| entry[*k].as_str())?.to_string();
    valid_id(&id).then_some(Resolved { id, last_write: None })
}

fn grok_newest_for(cwd: &str) -> Option<Resolved> {
    if cwd.is_empty() {
        return None;
    }
    let dir = grok_dir().join("sessions").join(&*urlencoding::encode(cwd));
    let (p, t) = newest_in(&dir, |_| true)?;
    let id = p.file_name()?.to_str()?.trim_end_matches(".json").trim_end_matches(".jsonl").to_string();
    valid_id(&id).then_some(Resolved { id, last_write: Some(t) })
}

// opencode: SQLite `~/.local/share/opencode/opencode.db`, table `session` (`id`, `directory`, `time_updated` ms).
fn opencode_newest_for(cwd: &str) -> Option<Resolved> {
    if cwd.is_empty() {
        return None;
    }
    let db = std::env::var_os("XDG_DATA_HOME").map_or_else(|| home().join(".local").join("share"), PathBuf::from).join("opencode").join("opencode.db");
    if !db.is_file() {
        return None;
    }
    let sql = format!("SELECT id, time_updated FROM session WHERE directory = '{}' AND parent_id IS NULL ORDER BY time_updated DESC LIMIT 1", cwd.replace('\'', "''"));
    let db = db.to_string_lossy().into_owned();
    let out = super::proc_scan::run_with_timeout("sqlite3", &["-readonly", "-separator", "\t", db.as_str(), sql.as_str()], Duration::from_secs(2))?;
    let (id, at) = out.lines().next()?.split_once('\t')?;
    let at = at.trim().parse::<u64>().ok().map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
    valid_id(id.trim()).then(|| Resolved { id: id.trim().to_string(), last_write: at })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn harness_table_carried_flags_drop_resume_ids_and_prompts() {
        assert_eq!(carried_flags("claude", &v(&["claude", "--dangerously-skip-permissions", "--resume", "abc"])), v(&["--dangerously-skip-permissions"]));
        assert_eq!(carried_flags("codex", &v(&["codex", "--yolo", "resume", "abc", "fix it"])), v(&["--yolo"]));
        assert_eq!(carried_flags("kimi", &v(&["kimi", "-S", "abc", "--model", "k2"])), v(&["--model", "k2"]));
        // Real Factory bot shape: every flag keeps its value.
        assert_eq!(
            carried_flags("claude", &v(&["/Users/x/.local/bin/claude", "--model", "opus", "--permission-mode", "bypassPermissions", "--settings", "/tmp/a b/s.json"])),
            v(&["--model", "opus", "--permission-mode", "bypassPermissions", "--settings", "/tmp/a b/s.json"])
        );
        // A prompt after a boolean flag, `-c`, `-p`, inline ids and forks never carry.
        assert_eq!(carried_flags("claude", &v(&["claude", "--verbose", "fix the login bug", "-c"])), v(&["--verbose"]));
        assert_eq!(carried_flags("claude", &v(&["claude", "--resume=abc", "--fork-session", "-p", "--add-dir", "/a", "/b", "--model=sonnet"])), v(&["--add-dir", "/a", "/b", "--model=sonnet"]));
        // codex `-c` is a config override and `-p` a profile: they carry, with their values.
        assert_eq!(carried_flags("codex", &v(&["codex", "-c", "model=\"o3\"", "-p", "work", "resume", "--last"])), v(&["-c", "model=\"o3\"", "-p", "work"]));
        assert_eq!(carried_flags("agy", &v(&["agy", "--conversation=11-22", "--dangerously-skip-permissions"])), v(&["--dangerously-skip-permissions"]));
        assert_eq!(carried_flags("gizzi", &v(&["/Applications/Allternit Desktop.app/Contents/Resources/bin/gizzi-code", "-s", "u1", "--yolo", "-m", "a/b", "/repo"])), v(&["--yolo", "-m", "a/b"]));
        assert_eq!(carried_flags("opencode", &v(&["opencode", "-s", "ses_1", "--fork", "--auto"])), v(&["--auto"]));
        assert_eq!(carried_flags("grok", &v(&["grok", "-r", "abc", "--always-approve", "-m", "grok-5"])), v(&["--always-approve", "-m", "grok-5"]));
        // A script CLI under node: the interpreter is not a flag.
        assert_eq!(carried_flags("qwen", &v(&["node", "--no-warnings", "/opt/homebrew/bin/qwen", "-y", "--resume", "q1"])), v(&["-y"]));
        // An unknown flag keeps a plain value, but not prompt text.
        assert_eq!(carried_flags("kimi", &v(&["kimi", "--theme", "dark", "--plan", "write tests please"])), v(&["--theme", "dark", "--plan"]));
        assert!(carried_flags("unknown", &v(&["x", "--a"])).is_empty());
    }

    #[test]
    fn harness_table_move_and_copy_argv() {
        let flags = v(&["--dangerously-skip-permissions"]);
        assert_eq!(move_argv("claude", "abc", &flags), Some(v(&["claude", "--dangerously-skip-permissions", "--resume", "abc"])));
        assert_eq!(copy_argv("claude", "abc", &flags), Some(v(&["claude", "--dangerously-skip-permissions", "--resume", "abc", "--fork-session"])));
        assert_eq!(move_argv("codex", "u1", &v(&["--yolo"])), Some(v(&["codex", "--yolo", "resume", "u1"])));
        assert_eq!(copy_argv("codex", "u1", &[]), Some(v(&["codex", "fork", "u1"])));
        assert_eq!(move_argv("kimi", "session_u1", &[]), Some(v(&["kimi", "--session", "session_u1"])));
        assert_eq!(copy_argv("kimi", "session_u1", &[]), Some(v(&["kimi", "fork", "session_u1"])));
        assert_eq!(move_argv("grok", "g1", &[]), Some(v(&["grok", "--resume", "g1"])));
        assert_eq!(copy_argv("grok", "g1", &[]), Some(v(&["grok", "--resume", "g1", "--fork-session"])));
        assert_eq!(move_argv("agy", "a1", &[]), Some(v(&["agy", "--conversation", "a1"])));
        assert_eq!(copy_argv("agy", "a1", &[]), None);
        assert_eq!(move_argv("gizzi", "z1", &[]), Some(v(&["gizzi", "--session", "z1"])));
        assert_eq!(copy_argv("gizzi", "z1", &[]), Some(v(&["gizzi", "--session", "z1", "--fork"])));
        assert_eq!(copy_argv("opencode", "ses_1", &[]), Some(v(&["opencode", "--session", "ses_1", "--fork"])));
        assert_eq!(move_argv("qwen", "q1", &v(&["-y"])), Some(v(&["qwen", "-y", "--resume", "q1"])));
        assert_eq!(copy_argv("qwen", "q1", &[]), Some(v(&["qwen", "--resume", "q1", "--fork-session"])));
        assert_eq!(new_argv("gizzi", &v(&["--yolo"])), Some(v(&["gizzi", "--yolo"])));
        assert_eq!(new_argv("qwen", &[]), Some(v(&["qwen"])));
        // No id, or one that could break out of an argv or a shell line: nothing to run.
        assert_eq!(move_argv("claude", "", &[]), None);
        assert_eq!(move_argv("claude", "a b", &[]), None);
        assert_eq!(move_argv("claude", "--help", &[]), None);
        assert_eq!(move_argv("claude", "x;rm", &[]), None);
    }

    #[test]
    fn harness_table_session_ids_from_argv_and_processes() {
        assert_eq!(argv_session_id("codex", &v(&["codex", "--yolo", "resume", "u1", "go on"])).as_deref(), Some("u1"));
        assert_eq!(argv_session_id("codex", &v(&["codex", "-c", "k=v", "resume", "u2"])).as_deref(), Some("u2"));
        assert_eq!(argv_session_id("codex", &v(&["codex", "fork", "u1"])), None);
        assert_eq!(argv_session_id("agy", &v(&["agy", "--conversation=a-1"])).as_deref(), Some("a-1"));
        assert_eq!(argv_session_id("kimi", &v(&["kimi", "-S", "session_x", "--model", "k2"])).as_deref(), Some("session_x"));
        assert_eq!(argv_session_id("opencode", &v(&["opencode", "-s", "ses_1", "--fork"])), None);
        assert_eq!(argv_session_id("claude", &v(&["claude", "--resume"])), None);
        assert_eq!(rollout_id(Path::new("/x/rollout-2026-11-19T21-37-19-01a1f707-b50a-7d20-9708-802a3590a5f3.jsonl")).as_deref(), Some("01a1f707-b50a-7d20-9708-802a3590a5f3"));

        let (h, argv) = harness_for_process("node", &v(&["node", "/opt/homebrew/bin/qwen", "-y"])).unwrap();
        assert_eq!((h.id, argv), ("qwen", v(&["/opt/homebrew/bin/qwen", "-y"])));
        assert_eq!(harness_for_process("kimi-code", &[]).map(|(h, _)| h.id), Some("kimi"));
        assert!(harness_for_process("node", &v(&["node", "/x/Ops/index.js"])).is_none());
        let codex = harness("codex").unwrap();
        assert!(is_interactive(codex, &v(&["codex", "--yolo"])));
        assert!(is_interactive(codex, &v(&["codex", "resume", "u1"])));
        assert!(!is_interactive(codex, &v(&["codex", "app-server", "--listen", "unix://"])));
        assert!(!is_interactive(codex, &v(&["codex", "exec", "do it"])));
        let gizzi = harness("gizzi").unwrap();
        assert!(is_interactive(gizzi, &v(&["gizzi-code", "--yolo", "/repo"])));
        assert!(!is_interactive(gizzi, &v(&["gizzi-code", "serve", "--port", "4096"])));
        assert!(!is_interactive(gizzi, &v(&["gizzi-code", "fabric-worker"])));
    }
}
