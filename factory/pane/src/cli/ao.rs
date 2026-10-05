//! ao — Allternit agent-orchestrator contract over the ao engine socket.
//!
//! Implements `ao spawn|send|watch|status|kill|doctor` with byte-level parity
//! to the bash scripts in `~/.claude/skills/agent-orchestrator/scripts/`
//! (same arguments, exit codes, stdout formats, semantics). The engine runs
//! as named session `ao`; the tmux script path remains the fallback.
//!
//! Design notes (P1 spike, docs/learnings/ALLTERNIT_RUNTIME_P1_NOTES.md):
//! - Dead panes/workspaces are removed engine-side, so liveness is a presence
//!   probe (workspace listed + pane present). A small registry at
//!   `~/.agent-orchestrator/state.json` is the tmux remain-on-exit analog that
//!   keeps DEAD sessions observable to `ao status`.
//! - Transcripts come from the additive PTY tee (`src/ao/transcript.rs`),
//!   requested via the `HERDR_AO_TRANSCRIPT` launch-env marker on the
//!   layout.apply pane node.
//! - Worktree add/remove shell out to git with the script's exact commands.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::ao_gate;
use crate::api::client::{ApiClient, ApiClientError};
use crate::api::schema::{
    EmptyParams, LayoutApplyParams, LayoutNode, LayoutPane, Method, PaneListParams,
    PaneReadParams, PaneSendInputParams, PingParams, ReadFormat, ReadSource, Request,
    WorkspaceCloseParams, WorkspaceCreateParams,
};

const USAGE_SPAWN: &str = "usage: ao spawn [--worktree] <slug> <repo-dir> <agent-cmd...>";
const USAGE_SEND: &str = "usage: ao send <slug> <prompt...> | ao send <slug> -f <file>";
const USAGE_WATCH: &str = "usage: ao watch <slug> <sentinel-file> [timeout] [interval]";
const USAGE_STATUS: &str = "usage: ao status [slug] [lines=25]";
const USAGE_KILL: &str = "usage: ao kill <slug> [--rm-worktree]";
// Dispatch-semantics subcommands (additive; not part of the bash parity
// contract — the ao_parity golden test compares only the six above).
const USAGE_QUEUE: &str =
    "usage: ao queue <slug> [prompt...] [--root <dir>] [--lead <id>] [--as-human]";
const USAGE_DRAIN: &str = "usage: ao drain <slug> [--all] [--root <dir>] [--lead <id>] [--as-human]";
const USAGE_RECOVER: &str = "usage: ao recover [slug] [--apply] [--lead <id>] [--as-human]";

const AO_DIR: &str = ".agent-orchestrator";
const LOGS_DIR: &str = "logs";
const STATE_FILE: &str = "state.json";

pub(super) fn run_ao_command(args: &[String]) -> std::io::Result<i32> {
    ensure_ao_session();
    let Some(action) = args.first().map(String::as_str) else {
        print_ao_help();
        return Ok(2);
    };
    let rest = &args[1..];
    match action {
        "spawn" => spawn(rest),
        "send" => send(rest),
        "watch" => watch(rest),
        "status" => status(rest),
        "kill" => kill(rest),
        "doctor" => doctor(rest),
        "queue" => queue(rest),
        "drain" => drain(rest),
        "recover" => recover(rest),
        "--help" | "-h" | "help" => {
            print_ao_help();
            Ok(0)
        }
        _ => {
            eprintln!("{USAGE_SPAWN}\n{USAGE_SEND}\n{USAGE_WATCH}\n{USAGE_STATUS}\n{USAGE_KILL}\nusage: ao doctor");
            Ok(2)
        }
    }
}

fn print_ao_help() {
    println!(
        "ao — Allternit agent orchestrator (engine session \"ao\")\n\
         \n\
         {USAGE_SPAWN}\n\
         {USAGE_SEND}\n\
         {USAGE_WATCH}\n\
         {USAGE_STATUS}\n\
         {USAGE_KILL}\n\
         usage: ao doctor\n\
         {USAGE_QUEUE}\n\
         {USAGE_DRAIN}\n\
         {USAGE_RECOVER}\n\
         \n\
         Dispatch semantics (additive to the parity contract):\n\
         - spawn records a dispatch-registry entry in ~/.agent-orchestrator/state.json\n\
           (runner command, worktree, branch, sentinel, owning lead, lifecycle,\n\
           mailbox depth). --lead <id> or AO_LEAD sets the owner.\n\
         - queue enqueues to the Rails Bus mailbox peer:ao-<slug> (queue-not-drop);\n\
           drain injects the oldest pending row through the verified ao-send paste\n\
           path and settles only after verified delivery. Single drainer per\n\
           recipient, owned by ao-engine. watch auto-drains when the pane is idle.\n\
         - queue/drain/send --queue/recover are fail-closed on ownership: the\n\
           caller must be the recorded lead, or pass --as-human.\n\
         - recover reconciles the registry against live tmux/engine sessions and\n\
           respawns dead-but-unfinished runners (dry-run default; --apply acts).\n\
         \n\
         Contract-compatible with the ao-* bash scripts; byte-level parity is\n\
         proven by tests/ao_parity/run.sh (golden side-by-side test)."
    );
}

/// Default the engine session to `ao` unless the user explicitly selected one
/// (`--session`, `HERDR_SOCKET_PATH`, or a pre-set `HERDR_SESSION` win).
fn ensure_ao_session() {
    if crate::session::explicit_session_requested() {
        return;
    }
    if std::env::var_os(crate::api::SOCKET_PATH_ENV_VAR).is_some() {
        return;
    }
    if std::env::var_os(crate::session::SESSION_ENV_VAR).is_none() {
        std::env::set_var(crate::session::SESSION_ENV_VAR, "ao");
    }
}

// ---------------------------------------------------------------------------
// RPC helpers
// ---------------------------------------------------------------------------

enum CallError {
    EngineDown,
    Rpc { code: String, message: String },
    Io(std::io::Error),
}

impl CallError {
    fn exit_already_reported(&self) -> i32 {
        match self {
            CallError::EngineDown => {
                eprintln!(
                    "error: ao engine is not running (socket {})",
                    crate::api::socket_path().display()
                );
                1
            }
            CallError::Rpc { message, .. } => {
                eprintln!("error: {message}");
                1
            }
            CallError::Io(err) => {
                eprintln!("error: {err}");
                1
            }
        }
    }
}

fn map_client_error(err: ApiClientError) -> CallError {
    match err {
        ApiClientError::Io(io_err)
            if matches!(
                io_err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            CallError::EngineDown
        }
        ApiClientError::Io(io_err) => CallError::Io(io_err),
        other => CallError::Io(std::io::Error::other(other)),
    }
}

fn call(client: &ApiClient, method: Method) -> Result<serde_json::Value, CallError> {
    let request = Request {
        id: "ao".into(),
        method,
    };
    let value = client.request_value(&request).map_err(map_client_error)?;
    if let Some(error) = value.get("error") {
        return Err(CallError::Rpc {
            code: error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            message: error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown engine error")
                .to_string(),
        });
    }
    Ok(value["result"].clone())
}

fn workspaces(client: &ApiClient) -> Result<Vec<serde_json::Value>, CallError> {
    let result = call(client, Method::WorkspaceList(EmptyParams {}))?;
    Ok(result["workspaces"].as_array().cloned().unwrap_or_default())
}

fn find_workspace(client: &ApiClient, label: &str) -> Result<Option<serde_json::Value>, CallError> {
    Ok(workspaces(client)?
        .into_iter()
        .find(|ws| ws["label"].as_str() == Some(label)))
}

/// Presence probe: the engine removes dead panes and their workspace, so
/// liveness is exactly "the workspace is listed".
fn session_alive(client: &ApiClient, session: &str) -> Result<bool, CallError> {
    Ok(find_workspace(client, session)?.is_some())
}

fn first_pane_cwd(client: &ApiClient, workspace_id: &str) -> Result<Option<String>, CallError> {
    let result = call(
        client,
        Method::PaneList(PaneListParams {
            workspace_id: Some(workspace_id.to_string()),
        }),
    )?;
    Ok(result["panes"]
        .as_array()
        .and_then(|panes| panes.first())
        .and_then(|pane| pane["cwd"].as_str())
        .map(str::to_string))
}

fn pane_read_text(client: &ApiClient, pane_id: &str, lines: u32) -> Result<String, CallError> {
    let result = call(
        client,
        Method::PaneRead(PaneReadParams {
            pane_id: pane_id.to_string(),
            source: ReadSource::Recent,
            lines: Some(lines),
            format: ReadFormat::Text,
            strip_ansi: true,
            intent: Default::default(),
        }),
    )?;
    Ok(result["read"]["text"].as_str().unwrap_or_default().to_string())
}

fn ensure_engine_running() -> std::io::Result<()> {
    let client = ApiClient::local();
    let request = Request {
        id: "ao:ping".into(),
        method: Method::Ping(PingParams::default()),
    };
    if client.request_value(&request).is_ok() {
        return Ok(());
    }
    crate::server::autodetect::spawn_server_daemon()?;
    crate::server::autodetect::wait_for_server_socket(
        &client.socket_path(),
        Duration::from_secs(15),
    )
}

// ---------------------------------------------------------------------------
// Dispatch registry (tmux remain-on-exit analog + dispatch spine)
//
// state.json shape: {"sessions": {"ao-<slug>": {...}}}. The first three
// fields (cwd/log/dead) are the original remain-on-exit analog; everything
// else is the dispatch registry extension (A1). Old readers ignore unknown
// fields (serde default), and every new field is optional or defaulted, so
// old files load cleanly and old binaries keep working against new files.
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct AoState {
    sessions: BTreeMap<String, AoSession>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AoSession {
    cwd: String,
    #[serde(default)]
    log: Option<String>,
    #[serde(default)]
    dead: bool,
    /// Exact relaunch command (`logs/ao-<slug>.cmd.sh`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runner: Option<String>,
    /// Isolated worktree dir when spawned with --worktree (else == cwd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    /// Worktree branch (`ao/<slug>`) when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    /// Sentinel NOTES file armed via `ao watch` (frontmatter status: done|blocked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sentinel: Option<String>,
    /// Owning lead identity (A3). Absent on pre-registry records — dispatch
    /// operations refuse fail-closed on those unless --as-human.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lead: Option<String>,
    /// running | dead | finished (free-form; absent on old records).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle: Option<String>,
    /// engine | tmux — which world the session was spawned in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    world: Option<String>,
    /// Cached mailbox depth for peer:ao-<slug>; refreshed by queue/drain.
    #[serde(default)]
    queued: u32,
}

fn ao_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(AO_DIR)
}

fn state_path() -> PathBuf {
    ao_home().join(STATE_FILE)
}

fn load_state() -> AoState {
    let Ok(file) = std::fs::File::open(state_path()) else {
        return AoState::default();
    };
    serde_json::from_reader(std::io::BufReader::new(file)).unwrap_or_default()
}

fn save_state(state: &AoState) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(file) = std::fs::File::create(path) else {
        return;
    };
    let _ = serde_json::to_writer_pretty(std::io::BufWriter::new(file), state);
}

// ---------------------------------------------------------------------------
// Ownership (A3 — fail-closed lead → runner)
// ---------------------------------------------------------------------------

/// Caller identity: explicit `--lead` flag > `AO_LEAD` env > USER/LOGNAME >
/// "human". Spawn records this as the owning lead; dispatch operations
/// resolve it the same way and compare.
fn caller_identity(lead_flag: Option<&str>) -> String {
    if let Some(lead) = lead_flag {
        if !lead.is_empty() {
            return lead.to_string();
        }
    }
    for var in ["AO_LEAD", "USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "human".to_string()
}

/// Fail-closed ownership gate for dispatch/mailbox/recovery operations.
/// The caller must be the recorded lead, or pass --as-human. An unknown
/// owner (pre-registry record with no lead) refuses — never a guess.
fn require_lead(
    session: &str,
    entry: Option<&AoSession>,
    caller: &str,
    as_human: bool,
    op: &str,
) -> Result<(), String> {
    if as_human {
        return Ok(());
    }
    match entry.and_then(|e| e.lead.as_deref()) {
        Some(lead) if lead == caller => Ok(()),
        Some(lead) => Err(format!(
            "{op} on {session} refused — runner is owned by lead '{lead}' (caller is '{caller}'); use --as-human to override"
        )),
        None => Err(format!(
            "{op} on {session} refused — no owning lead recorded; use --as-human to override"
        )),
    }
}

/// Parsed flags shared by the dispatch subcommands (queue/drain/recover).
#[derive(Default)]
struct DispatchFlags {
    root: Option<String>,
    lead: Option<String>,
    as_human: bool,
    all: bool,
    apply: bool,
    positional: Vec<String>,
}

fn take_dispatch_flags(args: &[String]) -> Result<DispatchFlags, i32> {
    let mut flags = DispatchFlags::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => match iter.next() {
                Some(value) => flags.root = Some(value.clone()),
                None => return Err(2),
            },
            "--lead" => match iter.next() {
                Some(value) => flags.lead = Some(value.clone()),
                None => return Err(2),
            },
            "--as-human" => flags.as_human = true,
            "--all" => flags.all = true,
            "--apply" => flags.apply = true,
            _ => flags.positional.push(arg.clone()),
        }
    }
    Ok(flags)
}

fn mailbox_root(flag: Option<&str>) -> std::io::Result<PathBuf> {
    crate::ao::peers::registry_root(flag)
}

/// Refresh the cached mailbox depth on a registry entry (best-effort).
fn refresh_queued(root: &Path, state: &mut AoState, session: &str) {
    if let Some(entry) = state.sessions.get_mut(session) {
        entry.queued = crate::ao::mailbox::pending_depth(root, session).unwrap_or(0) as u32;
    }
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

fn session_of(slug: &str) -> String {
    format!("ao-{slug}")
}

fn alnum(text: &str) -> String {
    text.bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(char::from)
        .collect()
}

pub(super) fn timestamp_now() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn command_path(binary: &str) -> Option<String> {
    if binary.contains(std::path::MAIN_SEPARATOR) {
        let path = Path::new(binary);
        return is_executable(path).then(|| binary.to_string());
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(binary);
        if is_executable(&candidate) {
            return Some(candidate.display().to_string());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// `command -v`-style lookup honoring PATH, used for the tmux-world guard.
fn run_quiet(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn eprintln_engine(err: &CallError) -> i32 {
    err.exit_already_reported()
}

// ---------------------------------------------------------------------------
// ao spawn
// ---------------------------------------------------------------------------

fn spawn(args: &[String]) -> std::io::Result<i32> {
    // Hidden parity probe: `ao spawn --gate-rewrite <line> <settings-file>`
    // mirrors `ao-spawn-gate --rewrite` (tests/ao_parity/gate_parity.sh).
    if args.first().map(String::as_str) == Some("--gate-rewrite") {
        if args.len() != 3 {
            eprintln!("usage: ao spawn --gate-rewrite <line> <settings-file>");
            return Ok(2);
        }
        let class = ao_gate::classify(&ao_gate::harness_of(&args[1]));
        let (line, notice) = ao_gate::rewrite(&args[1], class, &args[2]);
        if let Some(notice) = notice {
            eprintln!("{notice}");
        }
        use std::io::Write as _;
        let mut out = std::io::stdout();
        out.write_all(line.as_bytes())?;
        out.flush()?;
        return Ok(0);
    }
    let mut args = args;
    let mut worktree = false;
    let mut lead_flag: Option<String> = None;
    // Leading flags: --worktree (parity with the script) and --lead <id>
    // (dispatch ownership; additive — never present in parity fixtures).
    loop {
        match args.first().map(String::as_str) {
            Some("--worktree") => {
                worktree = true;
                args = &args[1..];
            }
            Some("--lead") => {
                let Some(value) = args.get(1) else {
                    eprintln!("{USAGE_SPAWN}");
                    return Ok(2);
                };
                lead_flag = Some(value.clone());
                args = &args[2..];
            }
            _ => break,
        }
    }
    if args.len() < 3 {
        eprintln!("{USAGE_SPAWN}");
        return Ok(2);
    }
    let slug = &args[0];
    let mut dir = args[1].clone();
    let agent_cmd = &args[2..];
    let session = session_of(slug);
    let lead = caller_identity(lead_flag.as_deref());
    let branch = worktree.then(|| format!("ao/{slug}"));

    let logs_dir = ao_home().join(LOGS_DIR);
    if let Err(err) = std::fs::create_dir_all(&logs_dir) {
        eprintln!("error: mkdir {}: {err}", logs_dir.display());
        return Ok(1);
    }
    let log = logs_dir.join(format!("{session}-{}.log", timestamp_now()));
    let runner = logs_dir.join(format!("{session}.cmd.sh"));

    // Duplicate guard, tmux world first (script-world collision) then engine.
    if command_path("tmux").is_some() {
        let mut check = Command::new("tmux");
        check
            .arg("has-session")
            .arg("-t")
            .arg(format!("={session}:"));
        if run_quiet(&mut check) {
            eprintln!(
                "error: session {session} already exists (ao status {slug} to inspect)"
            );
            return Ok(1);
        }
    }
    if ensure_engine_running().is_err() {
        eprintln!(
            "error: could not start the ao engine (session ao, socket {})",
            crate::api::socket_path().display()
        );
        return Ok(1);
    }
    let client = ApiClient::local();
    match find_workspace(&client, &session) {
        Ok(Some(_)) => {
            eprintln!("error: session {session} already exists (ao status {slug} to inspect)");
            return Ok(1);
        }
        Ok(None) => {}
        Err(err) => return Ok(eprintln_engine(&err)),
    }

    let gated: Option<ao_gate::Gated>;
    if worktree {
        // `ROOT=$(git -C "$DIR" rev-parse --show-toplevel)` — git's stderr and
        // exit code propagate exactly like the script's set -e.
        let output = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("rev-parse")
            .arg("--show-toplevel")
            .stderr(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            return Ok(output.status.code().unwrap_or(1));
        }
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let home = std::env::var("HOME").unwrap_or_default();
        if root == home {
            eprintln!("error: git root is $HOME — a worktree would checkout your whole home dir. Rerun without --worktree.");
            return Ok(1);
        }
        let root_path = PathBuf::from(&root);
        let wt = root_path
            .parent()
            .map(|parent| parent.join(format!("{}-ao-{slug}", root_path.file_name().unwrap_or_default().to_string_lossy())))
            .unwrap_or_else(|| PathBuf::from(format!("{root}-ao-{slug}")));
        // Spawn gate before any side effect (the script gates at the same point).
        gated = match ao_gate::gate(&session, &wt.display().to_string(), &agent_cmd.join(" "), &logs_dir, &ao_home()) {
            Ok(gated) => Some(gated),
            Err(()) => return Ok(1),
        };
        // stderr discarded to match the golden script's `>/dev/null 2>&1`
        // (the script stopped forwarding git's worktree noise 2026-09-09).
        let status = Command::new("git")
            .arg("-C")
            .arg(&root)
            .arg("worktree")
            .arg("add")
            .arg(&wt)
            .arg("-b")
            .arg(format!("ao/{slug}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Ok(status.code().unwrap_or(1));
        }
        dir = wt.display().to_string();
    } else {
        gated = match ao_gate::gate(&session, &dir, &agent_cmd.join(" "), &logs_dir, &ao_home()) {
            Ok(gated) => Some(gated),
            Err(()) => return Ok(1),
        };
    }
    let gated = gated.expect("both branches gate");

    // Runner file sidesteps quoting issues. Ungated harnesses keep the
    // engine's direct argv exec; gated ones run the rewritten runner line
    // through /bin/sh exactly like the script world.
    let _ = std::fs::write(&runner, format!("{}\n", gated.line));
    let pane_cmd: Vec<String> = if gated.class == ao_gate::GateClass::Ungated {
        agent_cmd.to_vec()
    } else {
        vec!["/bin/sh".to_string(), runner.display().to_string()]
    };

    let created = match call(
        &client,
        Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some(dir.clone()),
            focus: false,
            label: Some(session.clone()),
            env: Default::default(),
        }),
    ) {
        Ok(result) => result,
        Err(err) => return Ok(eprintln_engine(&err)),
    };
    let workspace_id = created["workspace"]["workspace_id"].as_str().unwrap_or_default();
    let tab_id = created["tab"]["tab_id"].as_str().unwrap_or_default();

    let mut env = std::collections::HashMap::new();
    env.insert(
        crate::ao::transcript::TRANSCRIPT_ENV_VAR.to_string(),
        log.display().to_string(),
    );
    let applied = call(
        &client,
        Method::LayoutApply(LayoutApplyParams {
            workspace_id: None,
            tab_id: Some(tab_id.to_string()),
            tab_label: None,
            focus: false,
            root: LayoutNode::Pane {
                pane: LayoutPane {
                    pane_id: None,
                    label: None,
                    cwd: Some(dir.clone()),
                    command: Some(pane_cmd),
                    env,
                },
            },
        }),
    );
    if let Err(err) = applied {
        let _ = call(
            &client,
            Method::WorkspaceClose(WorkspaceCloseParams {
                workspace_id: workspace_id.to_string(),
                close_group: true,
            }),
        );
        return Ok(eprintln_engine(&err));
    }

    std::thread::sleep(Duration::from_millis(500));
    if !session_alive(&client, &session).unwrap_or(false) {
        eprintln!("error: agent exited immediately — transcript tail:");
        if let Ok(content) = std::fs::read_to_string(&log) {
            // tail -5 semantics: split on \n only, keep any \r bytes.
            let mut tail: Vec<&str> = content.split('\n').collect();
            if tail.last().is_some_and(|line| line.is_empty()) {
                tail.pop();
            }
            let start = tail.len().saturating_sub(5);
            for line in &tail[start..] {
                eprintln!("{line}");
            }
        }
        let mut state = load_state();
        state.sessions.insert(
            session.clone(),
            AoSession {
                cwd: dir.clone(),
                log: Some(log.display().to_string()),
                dead: true,
                runner: Some(runner.display().to_string()),
                worktree: worktree.then(|| dir.clone()),
                branch: branch.clone(),
                sentinel: None,
                lead: Some(lead.clone()),
                lifecycle: Some("dead".to_string()),
                world: Some("engine".to_string()),
                queued: 0,
            },
        );
        save_state(&state);
        return Ok(1);
    }

    let mut state = load_state();
    state.sessions.insert(
        session.clone(),
        AoSession {
            cwd: dir.clone(),
            log: Some(log.display().to_string()),
            dead: false,
            runner: Some(runner.display().to_string()),
            worktree: worktree.then(|| dir.clone()),
            branch: branch.clone(),
            sentinel: None,
            lead: Some(lead.clone()),
            lifecycle: Some("running".to_string()),
            world: Some("engine".to_string()),
            queued: 0,
        },
    );
    save_state(&state);

    println!("{session} {dir} {}", log.display());
    Ok(0)
}

// ---------------------------------------------------------------------------
// ao send
// ---------------------------------------------------------------------------

fn send(args: &[String]) -> std::io::Result<i32> {
    let mut args = args;
    // --queue (additive, A2): immediate verified send is still attempted;
    // on a busy/unverifiable pane the prompt falls back to the Bus mailbox
    // instead of being dropped. Without the flag the behavior is byte-identical
    // to the bash contract.
    let mut queue_fallback = false;
    if args.first().map(String::as_str) == Some("--queue") {
        queue_fallback = true;
        args = &args[1..];
    }
    if args.len() < 2 {
        eprintln!("{USAGE_SEND}");
        return Ok(2);
    }
    let slug = &args[0];
    let session = session_of(slug);
    let prompt = if args[1] == "-f" {
        let Some(file) = args.get(2) else {
            eprintln!("{USAGE_SEND}");
            return Ok(2);
        };
        // PROMPT=$(cat "$2") — cat's stderr propagates; trailing newlines
        // are stripped like command substitution does.
        let output = Command::new("cat").arg(file).stderr(Stdio::inherit()).output()?;
        if !output.status.success() {
            return Ok(output.status.code().unwrap_or(1));
        }
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        while text.ends_with('\n') {
            text.pop();
        }
        text
    } else {
        args[1..].join(" ")
    };

    let client = ApiClient::local();
    let workspace = match find_workspace(&client, &session) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => {
            if queue_fallback {
                return enqueue_fallback(&session, &prompt);
            }
            eprintln!("error: no session {session}");
            return Ok(1);
        }
        Err(err) => return Ok(eprintln_engine(&err)),
    };
    let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default();

    let Some(marker) = prompt_marker(&prompt) else {
        eprintln!("error: prompt has no alphanumeric content");
        return Ok(2);
    };

    let pane = match first_pane_id(&client, workspace_id) {
        Ok(Some(pane)) => pane,
        Ok(None) => {
            eprintln!("error: no session {session}");
            return Ok(1);
        }
        Err(err) => return Ok(eprintln_engine(&err)),
    };

    match paste_and_verify(&client, &pane, &prompt, &marker) {
        Ok(true) => {
            println!("submitted to {session}");
            Ok(0)
        }
        Ok(false) => {
            if queue_fallback {
                return enqueue_fallback(&session, &prompt);
            }
            eprintln!("error: prompt not found in {session} pane after paste — cleared line with C-u, NOT submitted. Inspect with: tmux capture-pane -p -t {session}");
            Ok(1)
        }
        Err(err) => Ok(send_failure_exit(&session, &err)),
    }
}

/// Queue-not-drop fallback (A2): the immediate path could not verify
/// delivery, so the prompt goes to the durable Bus mailbox for the drainer.
/// Fail-closed on ownership like every dispatch operation (A3).
fn enqueue_fallback(session: &str, prompt: &str) -> std::io::Result<i32> {
    let root = mailbox_root(None)?;
    let caller = caller_identity(None);
    let state = load_state();
    if let Err(message) = require_lead(
        session,
        state.sessions.get(session),
        &caller,
        false,
        "send --queue",
    ) {
        eprintln!("error: {message}");
        return Ok(1);
    }
    match crate::ao::mailbox::enqueue(&root, session, &caller, prompt) {
        Ok((id, depth)) => {
            let mut state = load_state();
            refresh_queued(&root, &mut state, session);
            save_state(&state);
            println!(
                "queued {id} to {} (depth {depth}) — pane busy or unverifiable",
                crate::ao::mailbox::recipient_for(session)
            );
            Ok(0)
        }
        Err(err) => {
            eprintln!("error: {err}");
            Ok(1)
        }
    }
}

/// Marker loop token, verbatim from ao-send: last-40-alnum of the prompt.
/// Comparing alnum-only text is immune to TUI line-wrapping, input-box
/// border chars, and padding.
fn prompt_marker(prompt: &str) -> Option<String> {
    let stripped = alnum(prompt);
    let marker: String = stripped
        .chars()
        .skip(stripped.chars().count().saturating_sub(40))
        .collect();
    (!marker.is_empty()).then_some(marker)
}

fn first_pane_id(client: &ApiClient, workspace_id: &str) -> Result<Option<String>, CallError> {
    let result = call(
        client,
        Method::PaneList(PaneListParams {
            workspace_id: Some(workspace_id.to_string()),
        }),
    )?;
    Ok(result["panes"]
        .as_array()
        .and_then(|panes| panes.first())
        .and_then(|pane| pane["pane_id"].as_str())
        .map(str::to_string))
}

/// Verified bracketed-paste injection shared by `ao send` and the mailbox
/// drainer (A2 settle path): paste, poll the pane until the marker shows in
/// two consecutive captures (a single sighting can be a half-painted frame),
/// Enter only after verified landing. On a bad read-back the line is cleared
/// with C-u (NEVER C-c — that kills kimi). Ok(true) == Enter was sent.
fn paste_and_verify(
    client: &ApiClient,
    pane: &str,
    prompt: &str,
    marker: &str,
) -> Result<bool, CallError> {
    let send = |text: &str, keys: &[&str]| {
        call(
            client,
            Method::PaneSendInput(PaneSendInputParams {
                pane_id: pane.to_string(),
                text: text.to_string(),
                keys: keys.iter().map(|key| (*key).to_string()).collect(),
            }),
        )
    };

    // Bracketed paste; the engine wraps text server-side (never send raw).
    if let Err(err) = send(prompt, &[]) {
        let _ = send("", &["ctrl+u"]);
        return Err(err);
    }

    let deadline = std::time::SystemTime::now() + Duration::from_secs(5);
    let mut seen = false;
    let mut last: String;
    loop {
        last = match pane_read_text(client, pane, 80) {
            Ok(text) => alnum(&text),
            Err(_) => String::new(),
        };
        if last.contains(marker) {
            if seen {
                break;
            }
            seen = true;
        } else {
            seen = false;
        }
        if std::time::SystemTime::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if last.contains(marker) {
        let _ = send("", &["enter"]);
        Ok(true)
    } else {
        let _ = send("", &["ctrl+u"]);
        Ok(false)
    }
}

fn send_failure_exit(session: &str, err: &CallError) -> i32 {
    match err {
        CallError::Rpc { code, .. } if code == "pane_not_found" => {
            eprintln!("error: no session {session}");
            1
        }
        other => eprintln_engine(other),
    }
}

// ---------------------------------------------------------------------------
// ao watch
// ---------------------------------------------------------------------------

fn watch(args: &[String]) -> std::io::Result<i32> {
    if args.len() < 2 {
        eprintln!("{USAGE_WATCH}");
        return Ok(2);
    }
    let slug = &args[0];
    let session = session_of(slug);
    let sentinel = &args[1];
    let timeout = match parse_u64_arg(args.get(2), 3600) {
        Some(value) => value,
        None => return Ok(2),
    };
    let interval = match parse_u64_arg(args.get(3), 20) {
        Some(value) => value,
        None => return Ok(2),
    };
    let client = ApiClient::local();
    // Record the armed sentinel on the dispatch registry entry (A1) so
    // `ao recover` can tell dead-but-unfinished sessions from finished ones.
    let mut state = load_state();
    if let Some(entry) = state.sessions.get_mut(&session) {
        entry.sentinel = Some(sentinel.clone());
        save_state(&state);
    }
    let mut elapsed: u64 = 0;
    loop {
        if Path::new(sentinel).exists() {
            println!("DONE {sentinel}");
            return Ok(0);
        }
        match session_alive(&client, &session) {
            Ok(true) => {}
            // Engine unreachable is indistinguishable from "session gone".
            Ok(false) | Err(_) => {
                println!("PANE-DEAD {session} (agent exited or session gone; check log in ~/.agent-orchestrator/logs)");
                mark_dead(&session);
                return Ok(3);
            }
        }
        if elapsed >= timeout {
            println!("TIMEOUT after {timeout}s; sentinel absent, pane alive");
            return Ok(4);
        }
        // Mailbox auto-drain (A2): pending dispatch mail for this session is
        // injected through the verified paste path once the pane reads idle.
        // Silent when the mailbox is empty — the parity contract never sees it.
        auto_drain_tick(&client, &session);
        std::thread::sleep(Duration::from_secs(interval));
        elapsed += interval;
    }
}

/// One automatic drain attempt on the oldest pending mailbox row. Every
/// failure mode (no bus, ownership mismatch, busy pane, unverifiable paste)
/// is a silent skip: watch is a monitor, not a dispatcher, and the row stays
/// pending for an explicit `ao drain`.
fn auto_drain_tick(client: &ApiClient, session: &str) {
    let Ok(root) = mailbox_root(None) else {
        return;
    };
    let state = load_state();
    let caller = caller_identity(None);
    if require_lead(session, state.sessions.get(session), &caller, false, "drain").is_err() {
        return;
    }
    let Ok(rows) = crate::ao::mailbox::pending(&root, session) else {
        return;
    };
    let Some(row) = rows.first() else {
        return;
    };
    let text = crate::ao::mailbox::message_text(row);
    if matches!(drain_one(client, session, &text), Ok(true))
        && crate::ao::mailbox::settle(&root, row.id).is_ok()
    {
        println!("drained {} -> {session}", row.id);
        let mut state = load_state();
        refresh_queued(&root, &mut state, session);
        save_state(&state);
    }
}

fn parse_u64_arg(arg: Option<&String>, default: u64) -> Option<u64> {
    match arg {
        None => Some(default),
        Some(raw) => match raw.parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => {
                eprintln!("error: '{raw}' is not a number");
                None
            }
        },
    }
}

fn mark_dead(session: &str) {
    let mut state = load_state();
    if let Some(entry) = state.sessions.get_mut(session) {
        entry.dead = true;
        entry.lifecycle = Some("dead".to_string());
        save_state(&state);
    }
}

// ---------------------------------------------------------------------------
// ao status
// ---------------------------------------------------------------------------

fn status(args: &[String]) -> std::io::Result<i32> {
    // The ao contract shadows the engine's `status` word at the dispatcher
    // (see cli.rs). Engine status forms are rehomed here so both vocabularies
    // work: `ao status --json`, `ao status server [--json]`,
    // `ao status client [--json]`, and `ao status help`. Everything else
    // stays on the ao contract (`ao status [slug] [lines]`). The remote
    // machine machinery probes `status server --json`, so without this
    // rehome `ao machine add` cannot inspect a remote server.
    match args.first().map(String::as_str) {
        Some("--json") | Some("server") | Some("client") | Some("help") | Some("--help")
        | Some("-h") => return super::status::run_status_command(args),
        _ => {}
    }
    let client = ApiClient::local();
    if args.is_empty() {
        let mut rows: Vec<(String, String, String)> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        match workspaces(&client) {
            Ok(list) => {
                for ws in list {
                    let Some(label) = ws["label"].as_str() else {
                        continue;
                    };
                    if !label.starts_with("ao-") {
                        continue;
                    }
                    seen.insert(label.to_string());
                    let workspace_id = ws["workspace_id"].as_str().unwrap_or_default();
                    let cwd = first_pane_cwd(&client, workspace_id)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "-".to_string());
                    rows.push((label.to_string(), "alive".to_string(), cwd));
                }
            }
            Err(CallError::EngineDown) => {}
            Err(err) => return Ok(eprintln_engine(&err)),
        }
        let state = load_state();
        for (session, entry) in &state.sessions {
            if seen.contains(session) {
                continue;
            }
            if !session.starts_with("ao-") {
                continue;
            }
            rows.push((session.clone(), "DEAD".to_string(), entry.cwd.clone()));
        }
        if rows.is_empty() {
            println!("no ao-* sessions");
        } else {
            for (name, state_label, cwd) in rows {
                println!("{name}  {state_label}  {cwd}");
            }
        }
        return Ok(0);
    }

    let slug = &args[0];
    let session = session_of(slug);
    let lines = match parse_u64_arg(args.get(1), 25) {
        Some(value) => value as u32,
        None => return Ok(2),
    };
    let workspace = match find_workspace(&client, &session) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => {
            eprintln!("no session {session}");
            return Ok(1);
        }
        Err(err) => return Ok(eprintln_engine(&err)),
    };
    println!("== {session} (alive) — last {lines} lines ==");
    let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default();
    let panes = call(
        &client,
        Method::PaneList(PaneListParams {
            workspace_id: Some(workspace_id.to_string()),
        }),
    );
    let Some(pane) = panes.ok().and_then(|result| {
        result["panes"]
            .as_array()
            .and_then(|panes| panes.first())
            .cloned()
    }) else {
        eprintln!("no session {session}");
        return Ok(1);
    };
    let pane_id = pane["pane_id"].as_str().unwrap_or_default();
    match pane_read_text(&client, pane_id, lines) {
        Ok(text) => print!("{text}"),
        Err(err) => return Ok(eprintln_engine(&err)),
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// ao kill
// ---------------------------------------------------------------------------

fn kill(args: &[String]) -> std::io::Result<i32> {
    if args.is_empty() {
        eprintln!("{USAGE_KILL}");
        return Ok(2);
    }
    let slug = &args[0];
    let session = session_of(slug);
    let rm_wt = args.get(1).map(String::as_str).unwrap_or("");

    let client = ApiClient::local();
    let mut wt_dir = String::new();
    match find_workspace(&client, &session) {
        Ok(Some(workspace)) => {
            let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default();
            if let Ok(cwd) = first_pane_cwd(&client, workspace_id) {
                wt_dir = cwd.unwrap_or_default();
            }
            let close = |group: bool| {
                call(
                    &client,
                    Method::WorkspaceClose(WorkspaceCloseParams {
                        workspace_id: workspace_id.to_string(),
                        close_group: group,
                    }),
                )
            };
            match close(false) {
                Ok(_) => {}
                Err(CallError::Rpc { code, .. }) if code == "workspace_group_close_required" => {
                    if let Err(err) = close(true) {
                        return Ok(eprintln_engine(&err));
                    }
                }
                Err(err) => return Ok(eprintln_engine(&err)),
            }
            println!("killed {session}");
        }
        Ok(None) => {
            println!("no session {session} (already gone)");
        }
        Err(err) => return Ok(eprintln_engine(&err)),
    }

    let mut state = load_state();
    if state.sessions.remove(&session).is_some() {
        save_state(&state);
    }

    if rm_wt == "--rm-worktree" {
        if !wt_dir.is_empty() && wt_dir.ends_with(&format!("-ao-{slug}")) {
            let removed = Command::new("git")
                .arg("-C")
                .arg(&wt_dir)
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&wt_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
                || Command::new("git")
                    .arg("worktree")
                    .arg("remove")
                    .arg("--force")
                    .arg(&wt_dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false);
            if !removed {
                eprintln!("warn: could not remove worktree {wt_dir} — remove manually");
            }
            println!("removed worktree {wt_dir}");
        } else {
            eprintln!("warn: pane dir '{wt_dir}' does not look like an ao worktree — not removing");
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// ao queue — Bus mailbox enqueue / inspect (A2)
// ---------------------------------------------------------------------------

fn queue(args: &[String]) -> std::io::Result<i32> {
    let flags = match take_dispatch_flags(args) {
        Ok(flags) => flags,
        Err(code) => {
            eprintln!("{USAGE_QUEUE}");
            return Ok(code);
        }
    };
    let Some(slug) = flags.positional.first() else {
        eprintln!("{USAGE_QUEUE}");
        return Ok(2);
    };
    let session = session_of(slug);
    let root = mailbox_root(flags.root.as_deref())?;

    if flags.positional.len() < 2 {
        // Inspect form: list pending rows (read-only, no ownership needed).
        return match crate::ao::mailbox::pending(&root, &session) {
            Ok(rows) => {
                if rows.is_empty() {
                    println!(
                        "no pending messages for {}",
                        crate::ao::mailbox::recipient_for(&session)
                    );
                } else {
                    for row in &rows {
                        println!(
                            "{}  {}  {}",
                            row.id,
                            row.created_at,
                            crate::ao::mailbox::message_text(row)
                        );
                    }
                }
                Ok(0)
            }
            Err(err) => {
                eprintln!("error: {err}");
                Ok(1)
            }
        };
    }

    let caller = caller_identity(flags.lead.as_deref());
    let state = load_state();
    if let Err(message) = require_lead(
        &session,
        state.sessions.get(&session),
        &caller,
        flags.as_human,
        "queue",
    ) {
        eprintln!("error: {message}");
        return Ok(1);
    }
    let text = flags.positional[1..].join(" ");
    match crate::ao::mailbox::enqueue(&root, &session, &caller, &text) {
        Ok((id, depth)) => {
            let mut state = load_state();
            refresh_queued(&root, &mut state, &session);
            save_state(&state);
            println!(
                "queued {id} to {} (depth {depth})",
                crate::ao::mailbox::recipient_for(&session)
            );
            Ok(0)
        }
        Err(err) => {
            eprintln!("error: {err}");
            Ok(1)
        }
    }
}

// ---------------------------------------------------------------------------
// ao drain — settle verified, roll back free (A2)
//
// Exactly one drainer may exist per recipient: the Bus delivery status is
// global per recipient, so two concurrent drainers would race poll/settle.
// The drainer is owned by ao-engine (this subcommand plus the watch loop's
// auto-drain tick). Do not bolt a second drainer onto the HTTP inbox
// endpoint — it marks delivered on read, which is exactly the semantics this
// path exists to avoid.
// ---------------------------------------------------------------------------

fn drain(args: &[String]) -> std::io::Result<i32> {
    let flags = match take_dispatch_flags(args) {
        Ok(flags) => flags,
        Err(code) => {
            eprintln!("{USAGE_DRAIN}");
            return Ok(code);
        }
    };
    let Some(slug) = flags.positional.first() else {
        eprintln!("{USAGE_DRAIN}");
        return Ok(2);
    };
    let session = session_of(slug);
    let root = mailbox_root(flags.root.as_deref())?;
    let caller = caller_identity(flags.lead.as_deref());
    let state = load_state();
    if let Err(message) = require_lead(
        &session,
        state.sessions.get(&session),
        &caller,
        flags.as_human,
        "drain",
    ) {
        eprintln!("error: {message}");
        return Ok(1);
    }

    let client = ApiClient::local();
    let mut drained: u32 = 0;
    loop {
        let rows = match crate::ao::mailbox::pending(&root, &session) {
            Ok(rows) => rows,
            Err(err) => {
                eprintln!("error: {err}");
                return Ok(1);
            }
        };
        let Some(row) = rows.first() else {
            if drained == 0 {
                println!(
                    "no pending messages for {}",
                    crate::ao::mailbox::recipient_for(&session)
                );
            }
            break;
        };
        let text = crate::ao::mailbox::message_text(row);
        match drain_one(&client, &session, &text) {
            Ok(true) => {
                // Verified delivery — settle. Anything short of this leaves
                // the row pending (rollback for free).
                if let Err(err) = crate::ao::mailbox::settle(&root, row.id) {
                    eprintln!("error: {err}");
                    return Ok(1);
                }
                drained += 1;
                println!("drained {} -> {session}", row.id);
            }
            Ok(false) => {
                println!(
                    "pending {} — pane busy or unverifiable; left in mailbox",
                    row.id
                );
                break;
            }
            Err(err) => return Ok(eprintln_engine(&err)),
        }
        if !flags.all {
            break;
        }
    }
    let mut state = load_state();
    refresh_queued(&root, &mut state, &session);
    save_state(&state);
    Ok(0)
}

/// One drain attempt: pane must be present and read idle, then the message
/// goes through the same verified paste path as `ao send`. Ok(false) means
/// "do not settle" — the row stays pending.
fn drain_one(client: &ApiClient, session: &str, text: &str) -> Result<bool, CallError> {
    let Some(workspace) = find_workspace(client, session)? else {
        return Ok(false);
    };
    let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default();
    let Some(pane) = first_pane_id(client, workspace_id)? else {
        return Ok(false);
    };
    if !pane_idle(client, &pane)? {
        return Ok(false);
    }
    let Some(marker) = prompt_marker(text) else {
        return Ok(false);
    };
    paste_and_verify(client, &pane, text, &marker)
}

/// Idle probe: two consecutive captures 800ms apart must be byte-identical.
/// A busy agent mid-turn keeps repainting; an idle prompt does not.
fn pane_idle(client: &ApiClient, pane: &str) -> Result<bool, CallError> {
    let first = pane_read_text(client, pane, 80)?;
    std::thread::sleep(Duration::from_millis(800));
    let second = pane_read_text(client, pane, 80)?;
    Ok(first == second)
}

// ---------------------------------------------------------------------------
// ao recover — reconcile the registry against reality (A4)
//
// Dry-run by default: prints the plan; --apply respawns. A session is a
// recovery candidate when it is dead in BOTH worlds (tmux + engine) and has
// no `status: done` sentinel. Respawn replays the exact relaunch command
// (logs/ao-<slug>.cmd.sh), upgraded to the harness's agent-level resume argv
// via src/agent_resume.rs when the runner line carries a resumable session
// reference (codex resume <id>, claude --resume <id>, kimi --session/-S <id>,
// agy --conversation <id>, grok --resume <id>). Sentinel watchers are
// re-armed after a successful respawn.
// ---------------------------------------------------------------------------

fn recover(args: &[String]) -> std::io::Result<i32> {
    let flags = match take_dispatch_flags(args) {
        Ok(flags) => flags,
        Err(code) => {
            eprintln!("{USAGE_RECOVER}");
            return Ok(code);
        }
    };
    let only = flags.positional.first().map(|slug| session_of(slug));
    let caller = caller_identity(flags.lead.as_deref());
    let client = ApiClient::local();
    let state = load_state();
    let tmux_sessions = tmux_list_ao_sessions();

    let mut exit = 0;
    let mut planned = 0u32;
    for (session, entry) in &state.sessions {
        if let Some(only) = &only {
            if session != only {
                continue;
            }
        }
        let slug = session.strip_prefix("ao-").unwrap_or(session).to_string();
        let alive = tmux_has_session(session)
            || session_alive(&client, session).unwrap_or(false);
        if alive {
            if only.is_some() {
                println!("SKIP {session} — alive");
            }
            continue;
        }
        if let Some(status) = sentinel_status(entry.sentinel.as_deref()) {
            if status == "done" {
                println!("SKIP {session} — sentinel status: done (finished)");
                if flags.apply {
                    let mut state = load_state();
                    if let Some(entry) = state.sessions.get_mut(session) {
                        entry.dead = true;
                        entry.lifecycle = Some("finished".to_string());
                        save_state(&state);
                    }
                }
                continue;
            }
        }
        if let Err(message) = require_lead(
            session,
            Some(entry),
            &caller,
            flags.as_human,
            "recover",
        ) {
            println!("REFUSE {message}");
            exit = 1;
            continue;
        }
        let Some((argv, resumed)) = recover_argv(session, entry) else {
            println!("SKIP {session} — no runner command file");
            continue;
        };
        let world = entry.world.as_deref().unwrap_or("engine").to_string();
        planned += 1;
        let how = if resumed { "agent-level resume" } else { "verbatim relaunch" };
        println!(
            "PLAN respawn {session} [{world}] ({how}) cwd={} cmd={}",
            entry.cwd,
            argv.join(" ")
        );
        if flags.apply {
            match respawn(&client, session, &slug, &world, &entry.cwd, &argv) {
                Ok(log) => {
                    let mut state = load_state();
                    if let Some(entry) = state.sessions.get_mut(session) {
                        entry.dead = false;
                        entry.lifecycle = Some("running".to_string());
                        entry.log = Some(log.display().to_string());
                        save_state(&state);
                    }
                    println!("RECOVERED {session} — transcript {}", log.display());
                    if let Some(sentinel) = &entry.sentinel {
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = Command::new(exe)
                                .arg("watch")
                                .arg(&slug)
                                .arg(sentinel)
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                            println!("re-armed sentinel watch: ao watch {slug} {sentinel}");
                        }
                    }
                }
                Err(message) => {
                    eprintln!("error: respawn {session}: {message}");
                    exit = 1;
                }
            }
        }
    }

    for session in &tmux_sessions {
        if !state.sessions.contains_key(session) {
            println!("NOTE {session} — live in tmux but absent from the dispatch registry");
        }
    }
    if planned == 0 && exit == 0 {
        println!("nothing to recover");
    } else if planned > 0 && !flags.apply {
        println!("dry-run — rerun with --apply to respawn");
    }
    Ok(exit)
}

/// Sentinel frontmatter status (`status: done|blocked`), or None when the
/// sentinel is absent/unreadable — absence means unfinished.
fn sentinel_status(sentinel: Option<&str>) -> Option<String> {
    let content = std::fs::read_to_string(sentinel?).ok()?;
    for line in content.lines().take(20) {
        if let Some(value) = line.trim().strip_prefix("status:") {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Build the respawn argv from the runner command file, upgrading to the
/// harness's resume argv (src/agent_resume.rs) when the recorded command is
/// itself a resumable launch. Returns (argv, used_agent_resume).
fn recover_argv(session: &str, entry: &AoSession) -> Option<(Vec<String>, bool)> {
    let runner = entry
        .runner
        .clone()
        .unwrap_or_else(|| ao_home().join(LOGS_DIR).join(format!("{session}.cmd.sh")).display().to_string());
    let line = std::fs::read_to_string(runner).ok()?;
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        return None;
    }
    if let Some(resumed) = resume_argv_from(&argv) {
        return Some((resumed, true));
    }
    Some((argv, false))
}

/// Detect a resumable agent launch and rebuild it through
/// agent_resume::plan. Unknown harnesses return None (verbatim relaunch).
fn resume_argv_from(argv: &[String]) -> Option<Vec<String>> {
    let binary = Path::new(argv.first()?).file_name()?.to_str()?;
    let (source, agent, flag): (&str, &str, &str) = match binary {
        "claude" => ("herdr:claude", "claude", "--resume"),
        "codex" => ("herdr:codex", "codex", "resume"),
        "kimi" => ("herdr:kimi", "kimi", "--session"),
        "agy" => ("herdr:antigravity_cli", "agy", "--conversation"),
        "grok" => ("herdr:grok", "grok", "--resume"),
        _ => return None,
    };
    let session_id = if agent == "codex" {
        // codex resume <id> — positional, not a --flag.
        (argv.get(1).filter(|a| a.as_str() == "resume").and_then(|_| argv.get(2)))
            .filter(|id| !id.starts_with('-'))?
    } else {
        let position = argv.iter().position(|arg| arg == flag || (agent == "kimi" && arg == "-S"))?;
        argv.get(position + 1)?
    };
    let session_ref = crate::agent_resume::AgentSessionRef::id(session_id.clone())?;
    let plan = crate::agent_resume::plan(source, agent, &session_ref)?;
    Some(plan.argv)
}

/// Respawn a session in its recorded world. Returns the new transcript path.
fn respawn(
    client: &ApiClient,
    session: &str,
    slug: &str,
    world: &str,
    cwd: &str,
    argv: &[String],
) -> Result<PathBuf, String> {
    let logs_dir = ao_home().join(LOGS_DIR);
    std::fs::create_dir_all(&logs_dir).map_err(|err| format!("mkdir: {err}"))?;
    let log = logs_dir.join(format!("{session}-{}.log", timestamp_now()));
    let runner = logs_dir.join(format!("{session}.cmd.sh"));
    // Respawns go through the same spawn gate as `ao spawn` (a resume argv
    // is rebuilt without the gate flags; a verbatim runner is re-gated
    // idempotently).
    let gated = ao_gate::gate(session, cwd, &argv.join(" "), &logs_dir, &ao_home())
        .map_err(|()| "spawn gate refused the respawn".to_string())?;
    std::fs::write(&runner, format!("{}\n", gated.line))
        .map_err(|err| format!("write runner: {err}"))?;
    let pane_cmd: Vec<String> = if gated.class == ao_gate::GateClass::Ungated {
        argv.to_vec()
    } else {
        vec!["/bin/sh".to_string(), runner.display().to_string()]
    };

    if world == "tmux" {
        if command_path("tmux").is_none() {
            return Err("tmux not installed".to_string());
        }
        let inner = format!(
            "script -q '{}' /bin/sh '{}'",
            log.display(),
            runner.display()
        );
        let status = Command::new("tmux")
            .arg("new-session")
            .arg("-d")
            .arg("-s")
            .arg(session)
            .arg("-c")
            .arg(cwd)
            .arg(&inner)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|err| format!("tmux new-session: {err}"))?;
        if !status.success() {
            return Err("tmux new-session failed".to_string());
        }
        let mut opt = Command::new("tmux");
        opt.arg("set-option")
            .arg("-t")
            .arg(format!("={session}:"))
            .arg("remain-on-exit")
            .arg("on");
        let _ = run_quiet(&mut opt);
        std::thread::sleep(Duration::from_millis(500));
        if !tmux_has_session(session) {
            return Err(format!("respawned agent exited immediately — see {}", log.display()));
        }
        return Ok(log);
    }

    // Engine world: same workspace+layout dance as spawn, with the respawn
    // argv. The slug is only used for error text here.
    let _ = slug;
    ensure_engine_running().map_err(|err| format!("engine start: {err}"))?;
    let created = call(
        client,
        Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some(cwd.to_string()),
            focus: false,
            label: Some(session.to_string()),
            env: Default::default(),
        }),
    )
    .map_err(|err| format!("workspace create: {}", call_error_text(&err)))?;
    let workspace_id = created["workspace"]["workspace_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let tab_id = created["tab"]["tab_id"].as_str().unwrap_or_default();
    let mut env = std::collections::HashMap::new();
    env.insert(
        crate::ao::transcript::TRANSCRIPT_ENV_VAR.to_string(),
        log.display().to_string(),
    );
    if let Err(err) = call(
        client,
        Method::LayoutApply(LayoutApplyParams {
            workspace_id: None,
            tab_id: Some(tab_id.to_string()),
            tab_label: None,
            focus: false,
            root: LayoutNode::Pane {
                pane: LayoutPane {
                    pane_id: None,
                    label: None,
                    cwd: Some(cwd.to_string()),
                    command: Some(pane_cmd),
                    env,
                },
            },
        }),
    ) {
        let _ = call(
            client,
            Method::WorkspaceClose(WorkspaceCloseParams {
                workspace_id,
                close_group: true,
            }),
        );
        return Err(format!("layout apply: {}", call_error_text(&err)));
    }
    std::thread::sleep(Duration::from_millis(500));
    if !session_alive(client, session).unwrap_or(false) {
        return Err(format!("respawned agent exited immediately — see {}", log.display()));
    }
    Ok(log)
}

fn call_error_text(err: &CallError) -> String {
    match err {
        CallError::EngineDown => "engine down".to_string(),
        CallError::Rpc { message, .. } => message.clone(),
        CallError::Io(err) => err.to_string(),
    }
}

fn tmux_has_session(session: &str) -> bool {
    if command_path("tmux").is_none() {
        return false;
    }
    let mut check = Command::new("tmux");
    check
        .arg("has-session")
        .arg("-t")
        .arg(format!("={session}:"));
    run_quiet(&mut check)
}

fn tmux_list_ao_sessions() -> Vec<String> {
    if command_path("tmux").is_none() {
        return Vec::new();
    }
    let output = Command::new("tmux")
        .arg("list-sessions")
        .arg("-F")
        .arg("#{session_name}")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("ao-"))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// ao doctor
// ---------------------------------------------------------------------------

fn doctor(_args: &[String]) -> std::io::Result<i32> {
    let mut transport_ok = true;
    let mut usable = false;

    println!("ao-doctor: transport");
    match engine_status_line() {
        Ok(line) => println!("{line}"),
        Err(line) => {
            println!("{line}");
            transport_ok = false;
        }
    }
    for tool in ["tmux", "script", "git"] {
        match command_path(tool) {
            Some(path) => println!("  {tool}: OK ({path})"),
            None => {
                println!("  {tool}: MISSING — delegation cannot run without it");
                transport_ok = false;
            }
        }
    }

    println!("ao-doctor: executors");
    probe_executor("kimi", "kimi", &["--yolo"], None, "no headless: -p refuses --yolo/--auto", &mut usable);
    probe_executor(
        "codex",
        "codex",
        &["--dangerously-bypass-approvals-and-sandbox"],
        Some(&["exec"]),
        "",
        &mut usable,
    );
    probe_executor(
        "claude",
        "claude",
        &["--dangerously-skip-permissions"],
        Some(&["-p", "--dangerously-skip-permissions"]),
        "",
        &mut usable,
    );
    probe_executor("agy", "agy", &["--dangerously-skip-permissions"], None, "", &mut usable);

    // P7 harness section (spec binding 7): managed-dir health, per-tool
    // binary+pin match, license acceptance state, sync reachability. The
    // ao_parity harness strips everything from this header to the next
    // `ao-doctor: ` section (or EOF) — it is additive surface, not part of
    // the P1 parity contract. Exit codes are unchanged when the section is
    // green, including the "nothing installed yet" case.
    let harness = harness_doctor_section();

    if !transport_ok {
        println!("ao-doctor: TRANSPORT BROKEN");
        return Ok(2);
    }
    if usable {
        if !harness {
            println!("ao-doctor: HARNESS PROBLEMS");
            return Ok(3);
        }
        println!("ao-doctor: OK — at least one executor is usable");
        return Ok(0);
    }
    println!("ao-doctor: NO USABLE EXECUTORS");
    Ok(1)
}

/// Print the `ao-doctor: harness` section; true when green. Loading the
/// manifest can fail (corrupt AO_HARNESS_MANIFEST override) — reported as a
/// problem rather than panicking inside doctor.
fn harness_doctor_section() -> bool {
    println!("ao-doctor: harness");
    let manifest = match crate::ao::harness::load_manifest_for_doctor() {
        Ok(manifest) => manifest,
        Err(err) => {
            println!("  manifest: UNREADABLE ({err})");
            return false;
        }
    };
    let report = crate::ao::harness::doctor_for_cli(&manifest);
    println!("  managed dir: {} [{}]", report.root.display(), if report.root.exists() { "exists" } else { "absent" });
    for row in &report.rows {
        if row.detail.is_empty() {
            println!("  {}: {}", row.tool, row.status);
        } else {
            println!("  {}: {} — {}", row.tool, row.status, row.detail);
        }
    }
    report.ok
}

fn engine_status_line() -> Result<String, String> {
    let client = ApiClient::local();
    let socket = client.socket_path().display().to_string();
    let request = Request {
        id: "ao:doctor".into(),
        method: Method::Ping(PingParams::default()),
    };
    match client.request_value(&request) {
        Ok(value) => {
            let result = &value["result"];
            let protocol = result["protocol"].as_u64().unwrap_or(0);
            let version = result["version"].as_str().unwrap_or("unknown");
            Ok(format!("  ao-engine: OK (socket {socket}, protocol {protocol}, {version})"))
        }
        Err(ApiClientError::Io(err))
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(format!(
                "  ao-engine: MISSING — start it with: ao spawn <slug> <dir> <cmd...> (or: ao --session ao server) [{socket}]"
            ))
        }
        Err(err) => Err(format!(
            "  ao-engine: STALE (socket present but ping failed: {err}) [{socket}]"
        )),
    }
}

/// Verbatim port of the script's probe(): --help substring flag checks plus a
/// --version line, byte-identical output formatting.
#[allow(clippy::too_many_arguments)]
fn probe_executor(
    vendor: &str,
    binary: &str,
    interactive_flags: &[&str],
    headless_flags: Option<&[&str]>,
    note: &str,
    usable: &mut bool,
) {
    let Some(_path) = command_path(binary) else {
        println!("  {vendor} ({binary}): not installed");
        return;
    };
    let help = {
        let mut child = match Command::new(binary)
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                println!("  {vendor} ({binary}): installed interactive=no headless=n/a");
                return;
            }
        };
        let mut output = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_to_end(&mut output);
        }
        let mut stderr = Vec::new();
        if let Some(mut err_pipe) = child.stderr.take() {
            let _ = err_pipe.read_to_end(&mut stderr);
        }
        let _ = child.wait();
        output.extend_from_slice(&stderr);
        String::from_utf8_lossy(&output[..output.len().min(200_000)]).into_owned()
    };
    let missing = |flags: &[&str]| -> String {
        flags
            .iter()
            .filter(|flag| !help.contains(**flag))
            .fold(String::new(), |mut acc, flag| {
                acc.push(' ');
                acc.push_str(flag);
                acc
            })
    };
    let missing_i = missing(interactive_flags);
    let missing_h = headless_flags.map(missing).unwrap_or_default();

    let i_ok = missing_i.is_empty();
    let h_ok = match headless_flags {
        None => "n/a",
        Some(_) if missing_h.is_empty() => "yes",
        Some(_) => "no",
    };
    if i_ok {
        *usable = true;
    }
    if h_ok == "yes" {
        *usable = true;
    }

    let mut line = format!(
        "  {vendor} ({binary}): installed interactive={} headless={h_ok}",
        if i_ok { "yes" } else { "no" }
    );
    let version = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8(output.stdout)
                .ok()
                .and_then(|text| text.lines().next().map(str::to_string))
        })
        .filter(|text| !text.is_empty());
    if let Some(version) = version {
        line.push_str(&format!(" version={version}"));
    }
    if !missing_i.is_empty() {
        line.push_str(&format!(" missing-interactive:{missing_i}"));
    }
    if !missing_h.is_empty() {
        line.push_str(&format!(" missing-headless:{missing_h}"));
    }
    if !note.is_empty() {
        line.push_str(&format!(" ({note})"));
    }
    println!("{line}");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with_lead(lead: Option<&str>) -> AoSession {
        AoSession {
            cwd: "/tmp".to_string(),
            log: None,
            dead: false,
            runner: None,
            worktree: None,
            branch: None,
            sentinel: None,
            lead: lead.map(str::to_string),
            lifecycle: Some("running".to_string()),
            world: Some("engine".to_string()),
            queued: 0,
        }
    }

    #[test]
    fn ownership_is_fail_closed() {
        let owned = entry_with_lead(Some("lead-a"));
        // Matching lead passes.
        assert!(require_lead("ao-x", Some(&owned), "lead-a", false, "queue").is_ok());
        // Mismatched caller refuses.
        assert!(require_lead("ao-x", Some(&owned), "lead-b", false, "queue").is_err());
        // --as-human overrides a mismatch.
        assert!(require_lead("ao-x", Some(&owned), "lead-b", true, "queue").is_ok());
        // Unknown owner (pre-registry record) refuses — never a guess.
        let unknown = entry_with_lead(None);
        assert!(require_lead("ao-x", Some(&unknown), "lead-a", false, "queue").is_err());
        assert!(require_lead("ao-x", None, "lead-a", false, "queue").is_err());
        // --as-human overrides the unknown-owner refusal too.
        assert!(require_lead("ao-x", Some(&unknown), "lead-a", true, "queue").is_ok());
    }

    #[test]
    fn caller_identity_prefers_flag_then_env_then_human() {
        assert_eq!(caller_identity(Some("explicit")), "explicit");
        // Env resolution is environment-dependent; only the fallback shape is
        // asserted: never empty.
        assert!(!caller_identity(None).is_empty());
    }

    #[test]
    fn old_state_files_still_load() {
        // The pre-dispatch-registry shape: only cwd/log/dead.
        let old = r#"{"sessions":{"ao-x":{"cwd":"/tmp","log":"/tmp/x.log","dead":false}}}"#;
        let state: AoState = serde_json::from_str(old).unwrap();
        let entry = &state.sessions["ao-x"];
        assert_eq!(entry.cwd, "/tmp");
        assert!(!entry.dead);
        assert_eq!(entry.lead, None);
        assert_eq!(entry.queued, 0);
        assert_eq!(entry.lifecycle, None);
    }

    #[test]
    fn new_state_files_keep_legacy_keys() {
        let mut state = AoState::default();
        state
            .sessions
            .insert("ao-x".to_string(), entry_with_lead(Some("lead-a")));
        let text = serde_json::to_string(&state).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entry = &value["sessions"]["ao-x"];
        // Legacy keys intact for old readers.
        assert_eq!(entry["cwd"], "/tmp");
        assert_eq!(entry["dead"], false);
        assert!(entry.as_object().unwrap().contains_key("log"));
        // New keys present.
        assert_eq!(entry["lead"], "lead-a");
        assert_eq!(entry["queued"], 0);
        // Old readers ignore unknown fields: deserialize with only the
        // legacy shape visible.
        #[derive(Deserialize)]
        struct LegacySession {
            #[allow(dead_code)]
            cwd: String,
            #[allow(dead_code)]
            #[serde(default)]
            log: Option<String>,
            #[allow(dead_code)]
            #[serde(default)]
            dead: bool,
        }
        #[derive(Deserialize)]
        struct LegacyState {
            sessions: BTreeMap<String, LegacySession>,
        }
        let legacy: LegacyState = serde_json::from_str(&text).unwrap();
        assert!(legacy.sessions.contains_key("ao-x"));
    }

    #[test]
    fn prompt_marker_matches_script_semantics() {
        // Last 40 alnum chars; non-alnum stripped.
        let marker = prompt_marker("hello, world!").unwrap();
        assert_eq!(marker, "helloworld");
        assert!(prompt_marker("!!! ---").is_none());
        let long = "x".repeat(100);
        assert_eq!(prompt_marker(&long).unwrap().len(), 40);
    }

    #[test]
    fn resume_argv_detection() {
        assert_eq!(
            resume_argv_from(&["codex".into(), "resume".into(), "codex-id".into()]),
            Some(vec!["codex".to_string(), "resume".to_string(), "codex-id".to_string()])
        );
        assert_eq!(
            resume_argv_from(&["claude".into(), "--resume".into(), "claude-id".into()]),
            Some(vec!["claude".to_string(), "--resume".to_string(), "claude-id".to_string()])
        );
        assert_eq!(
            resume_argv_from(&["kimi".into(), "-S".into(), "kimi-id".into()]),
            Some(vec!["kimi".to_string(), "--session".to_string(), "kimi-id".to_string()])
        );
        // Non-agent commands are not resumable.
        assert!(resume_argv_from(&["cat".into()]).is_none());
        // codex resume --last has no explicit session id.
        assert!(resume_argv_from(&["codex".into(), "resume".into(), "--last".into()]).is_none());
    }

    #[test]
    fn sentinel_status_parses_frontmatter() {
        let dir = std::env::temp_dir().join(format!("ao-sentinel-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let done = dir.join("done.md");
        std::fs::write(&done, "---\nstatus: done\n---\nnotes\n").unwrap();
        assert_eq!(
            sentinel_status(Some(done.to_str().unwrap())).as_deref(),
            Some("done")
        );
        let blocked = dir.join("blocked.md");
        std::fs::write(&blocked, "---\nstatus: blocked\n---\n").unwrap();
        assert_eq!(
            sentinel_status(Some(blocked.to_str().unwrap())).as_deref(),
            Some("blocked")
        );
        assert_eq!(sentinel_status(Some(dir.join("absent.md").to_str().unwrap())), None);
        assert_eq!(sentinel_status(None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
