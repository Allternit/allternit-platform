//! The pane engine's socket helpers for agent panes, and `allternit-factory
//! pane doctor`.
//!
//! The Factory engine reaches agent panes through these calls
//! ([`crate::factory_backend`]): the workspace list, pane reads, and the
//! verified paste a send or a mailbox drain uses. Agent sessions are
//! started, sent to, recovered and stopped with `allternit-factory agents …`
//! and `orchestration …` (the engine's one spawn path); the pane engine has no
//! verbs of its own for them.
//!
//! The engine runs as the named session `ao` (the label is data: live panes
//! and the registry key on `ao-<slug>`). Transcripts come from the PTY tee in
//! `src/ao/transcript.rs`, asked for through the launch-env marker on the
//! layout.apply pane node.

use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::api::client::{ApiClient, ApiClientError};
use crate::api::schema::{
    EmptyParams, Method, PaneReadParams, PaneSendInputParams, PingParams, ReadFormat, ReadSource,
    Request,
};

/// `allternit-factory pane doctor`.
pub(super) fn run_doctor_command(args: &[String]) -> std::io::Result<i32> {
    ensure_ao_session();
    match args.first().map(String::as_str) {
        None => doctor(),
        Some("--help" | "-h" | "help") => {
            println!("usage: allternit-factory pane doctor\n\nChecks the pane engine socket, git, and the agent harnesses (installed, launch flags, managed harness dir).");
            Ok(0)
        }
        Some(_) => {
            eprintln!("usage: allternit-factory pane doctor");
            Ok(2)
        }
    }
}


/// Default the engine session to `ao` unless the user explicitly selected one
/// (`--session`, `HERDR_SOCKET_PATH`, or a pre-set `HERDR_SESSION` win).
pub(crate) fn ensure_ao_session() {
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

pub(crate) enum CallError {
    EngineDown,
    Rpc { code: String, message: String },
    Io(std::io::Error),
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

pub(crate) fn call(client: &ApiClient, method: Method) -> Result<serde_json::Value, CallError> {
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

pub(crate) fn workspaces(client: &ApiClient) -> Result<Vec<serde_json::Value>, CallError> {
    let result = call(client, Method::WorkspaceList(EmptyParams {}))?;
    Ok(result["workspaces"].as_array().cloned().unwrap_or_default())
}

pub(crate) fn find_workspace(client: &ApiClient, label: &str) -> Result<Option<serde_json::Value>, CallError> {
    Ok(workspaces(client)?
        .into_iter()
        .find(|ws| ws["label"].as_str() == Some(label)))
}

pub(crate) fn pane_read_text(client: &ApiClient, pane_id: &str, lines: u32) -> Result<String, CallError> {
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

/// The factory engine's one gate before it spawns: whatever answers the pane
/// socket must be fit to spawn terminals.
///
/// A ping that merely succeeds is not enough. The Desktop updater deletes
/// superseded runtime dirs, and the pane server process survives the deletion:
/// it keeps answering the socket while every pane it spawns dies instantly
/// (the deleted build's terminal permissions are gone), which surfaces far
/// away as a bare `workspace w6 not found`. Reject that server here, with the
/// fix, instead of letting the spawn fail downstream.
pub(crate) fn ensure_engine_running() -> std::io::Result<()> {
    let client = ApiClient::local();
    let request = Request {
        id: "ao:ping".into(),
        method: Method::Ping(PingParams::default()),
    };
    if let Ok(value) = client.request_value(&request) {
        return validate_ping_for_factory(&value["result"], &client.socket_path());
    }
    crate::server::autodetect::spawn_server_daemon()?;
    crate::server::autodetect::wait_for_server_socket(
        &client.socket_path(),
        Duration::from_secs(15),
    )
}

/// Old pane servers predate the `server` block (no `exe_deleted` field): a
/// missing field means "cannot tell", not "stale" — only an explicit `true`
/// gates.
fn validate_ping_for_factory(result: &serde_json::Value, socket: &Path) -> std::io::Result<()> {
    if result["server"]["exe_deleted"].as_bool() != Some(true) {
        return Ok(());
    }
    let pid = result["server"]["pid"]
        .as_u64()
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let exe = result["server"]["exe"]
        .as_str()
        .unwrap_or("the deleted build's executable");
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!(
            "the pane engine answering {} (pid {pid}) runs from a deleted app build ({exe} no longer exists on disk) — its terminal permissions are dead, so every pane it spawns dies instantly. Restart Allternit Desktop to respawn the pane engine from the current build; without Desktop, run `allternit-factory pane server stop`, then retry (the pane engine respawns from the current binary on demand). Stopping exits live pane processes.",
            socket.display(),
        ),
    ))
}

fn alnum(text: &str) -> String {
    text.bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(char::from)
        .collect()
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


/// The paste-verification marker: the last 40 alphanumerics of the prompt.
/// Comparing alnum-only text is immune to TUI line-wrapping, input-box
/// border chars, and padding.
pub(crate) fn prompt_marker(prompt: &str) -> Option<String> {
    let stripped = alnum(prompt);
    let marker: String = stripped
        .chars()
        .skip(stripped.chars().count().saturating_sub(40))
        .collect();
    (!marker.is_empty()).then_some(marker)
}


/// Verified bracketed-paste injection shared by an engine send and the
/// mailbox drain (which settles only on `Ok(true)`): paste, poll the pane until the marker shows in
/// two consecutive captures (a single sighting can be a half-painted frame),
/// Enter only after verified landing. On a bad read-back the line is cleared
/// with C-u (NEVER C-c — that kills kimi). Ok(true) == Enter was sent.
pub(crate) fn paste_and_verify(
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


/// Idle probe: two consecutive captures 800ms apart must be byte-identical.
/// A busy agent mid-turn keeps repainting; an idle prompt does not.
pub(crate) fn pane_idle(client: &ApiClient, pane: &str) -> Result<bool, CallError> {
    let first = pane_read_text(client, pane, 80)?;
    std::thread::sleep(Duration::from_millis(800));
    let second = pane_read_text(client, pane, 80)?;
    Ok(first == second)
}

// ---------------------------------------------------------------------------
// allternit-factory pane doctor
// ---------------------------------------------------------------------------

fn doctor() -> std::io::Result<i32> {
    let mut transport_ok = true;
    let mut usable = false;

    println!("doctor: transport");
    match engine_status_line() {
        Ok(line) => println!("{line}"),
        Err(line) => {
            println!("{line}");
            transport_ok = false;
        }
    }
    for tool in ["git"] {
        match command_path(tool) {
            Some(path) => println!("  {tool}: OK ({path})"),
            None => {
                println!("  {tool}: MISSING — delegation cannot run without it");
                transport_ok = false;
            }
        }
    }

    println!("doctor: executors");
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

    // Harness section: managed-dir health, per-tool binary+pin match,
    // license acceptance state, sync reachability. Exit codes are unchanged
    // when it is green, including the "nothing installed yet" case.
    let harness = harness_doctor_section();

    if !transport_ok {
        println!("doctor: TRANSPORT BROKEN");
        return Ok(2);
    }
    if usable {
        if !harness {
            println!("doctor: HARNESS PROBLEMS");
            return Ok(3);
        }
        println!("doctor: OK — at least one executor is usable");
        return Ok(0);
    }
    println!("doctor: NO USABLE EXECUTORS");
    Ok(1)
}

/// Print the `doctor: harness` section; true when green. Loading the
/// manifest can fail (corrupt ALLTERNIT_FACTORY_HARNESS_MANIFEST override) — reported as a
/// problem rather than panicking inside doctor.
fn harness_doctor_section() -> bool {
    println!("doctor: harness");
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
            let server = &result["server"];
            let exe = server["exe"].as_str().unwrap_or("");
            let identity = match (server["pid"].as_u64(), exe) {
                (Some(pid), exe) if !exe.is_empty() => format!(", pid {pid}, exe {exe}"),
                (Some(pid), _) => format!(", pid {pid}"),
                _ => String::new(),
            };
            if server["exe_deleted"].as_bool() == Some(true) {
                return Err(format!(
                    "  pane engine: STALE BUILD (socket {socket}, protocol {protocol}, {version}{identity}) — the executable was deleted (Desktop updater); spawned panes die instantly. Restart Allternit Desktop."
                ));
            }
            Ok(format!("  pane engine: OK (socket {socket}, protocol {protocol}, {version}{identity})"))
        }
        Err(ApiClientError::Io(err))
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(format!(
                "  pane engine: MISSING — it starts with the first agent (`allternit-factory agents up`), or run: allternit-factory pane --session ao server [{socket}]"
            ))
        }
        Err(err) => Err(format!(
            "  pane engine: STALE (socket present but ping failed: {err}) [{socket}]"
        )),
    }
}

/// Probe one harness: --help substring flag checks plus a --version line.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_marker_is_the_last_40_alphanumerics() {
        // Non-alnum stripped (immune to wrapping and input-box borders).
        assert_eq!(prompt_marker("hello, world!").unwrap(), "helloworld");
        assert!(prompt_marker("!!! ---").is_none());
        assert_eq!(prompt_marker(&"x".repeat(100)).unwrap().len(), 40);
    }

    #[test]
    fn factory_ping_rejects_a_deleted_build() {
        let result = serde_json::json!({
            "version": "0.5.5",
            "protocol": 2,
            "server": { "pid": 80193, "exe": "/Applications/Allternit Desktop.app/…/allternit-factory", "exe_deleted": true },
        });
        let err = validate_ping_for_factory(&result, Path::new("/tmp/ao.sock")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("deleted app build"), "unexpected: {message}");
        assert!(message.contains("80193"), "unexpected: {message}");
        assert!(message.contains("Restart Allternit Desktop"), "unexpected: {message}");
    }

    #[test]
    fn factory_ping_accepts_a_healthy_server() {
        let result = serde_json::json!({
            "version": "0.5.5",
            "protocol": 2,
            "server": { "pid": 56472, "exe": "/Applications/Allternit Desktop.app/…/allternit-factory", "exe_deleted": false },
        });
        assert!(validate_ping_for_factory(&result, Path::new("/tmp/ao.sock")).is_ok());
    }

    #[test]
    fn factory_ping_treats_missing_server_block_as_unknown() {
        // Pane servers older than ServerPingInfo don't send `server`:
        // "cannot tell" must not become "stale".
        let result = serde_json::json!({ "version": "0.5.5", "protocol": 2 });
        assert!(validate_ping_for_factory(&result, Path::new("/tmp/ao.sock")).is_ok());
    }
}
