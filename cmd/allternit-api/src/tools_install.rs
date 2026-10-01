//! Runtime install of CLI tools / agents through `allternit-tools` (plan D2).
//!
//! The installer itself is the zero-dependency Node script in
//! `tools/allternit-tools/` (manifest + pins + verify + terms gate). This
//! module only finds it, runs it for one tool with `--json`, and keeps the
//! progress events in memory so the UI can poll
//! `GET /providers/:id/install/status`.
//!
//! Where the installer comes from:
//! - Desktop sets `ALLTERNIT_TOOLS_SCRIPT` (+ `ALLTERNIT_TOOLS_NODE`, its own
//!   Electron binary run as Node) for the API sidecar.
//! - Cloud computers / VPS images put an `allternit-tools` launcher on PATH.
//! - Anything else (the shared cloud API) has neither, so install is
//!   reported as unavailable instead of running on a shared server.
//!   `ALLTERNIT_TOOLS_INSTALL_DISABLED=1` force-disables it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_EVENTS: usize = 400;

#[derive(Debug, Clone)]
pub struct InstallerCmd {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallJob {
    pub tool: String,
    /// running | succeeded | failed
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    /// Last error event's code (e.g. terms_required, npm_failed, no_installer).
    pub error: Option<String>,
    pub message: Option<String>,
    pub events: Vec<Value>,
}

fn jobs() -> &'static Mutex<HashMap<String, InstallJob>> {
    static JOBS: OnceLock<Mutex<HashMap<String, InstallJob>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Tool ids are manifest ids / aliases: lowercase, digits, `-`, `.`, `_`.
/// Anything else (notably a leading `-`) is rejected so an id can never be
/// read as an installer flag.
pub fn valid_tool_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_'))
}

/// `~/.allternit/tools/bin` (or `$ALLTERNIT_TOOLS_PREFIX/bin`).
pub fn tools_bin_dir() -> PathBuf {
    if let Ok(prefix) = std::env::var("ALLTERNIT_TOOLS_PREFIX") {
        if !prefix.is_empty() {
            return PathBuf::from(prefix).join("bin");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".allternit").join("tools").join("bin")
}

/// Locate the installer, or None when this runtime must not install tools.
pub fn installer_command() -> Option<InstallerCmd> {
    if std::env::var("ALLTERNIT_TOOLS_INSTALL_DISABLED").as_deref() == Ok("1") {
        return None;
    }
    if let Ok(script) = std::env::var("ALLTERNIT_TOOLS_SCRIPT") {
        let script = PathBuf::from(script);
        if script.is_file() {
            let node = std::env::var("ALLTERNIT_TOOLS_NODE")
                .ok()
                .map(PathBuf::from)
                .filter(|p| p.is_file())
                .or_else(|| crate::provider_routes::command_on_path("node"))?;
            return Some(InstallerCmd {
                program: node,
                args: vec![script.to_string_lossy().into_owned()],
                // Electron's binary only behaves as Node with this set; real
                // Node ignores it.
                env: vec![("ELECTRON_RUN_AS_NODE".into(), "1".into())],
            });
        }
    }
    let launcher = tools_bin_dir().join("allternit-tools");
    if launcher.is_file() {
        return Some(InstallerCmd { program: launcher, args: vec![], env: vec![] });
    }
    crate::provider_routes::command_on_path("allternit-tools")
        .map(|program| InstallerCmd { program, args: vec![], env: vec![] })
}

pub fn job(tool: &str) -> Option<InstallJob> {
    jobs().lock().ok()?.get(tool).cloned()
}

fn update(tool: &str, f: impl FnOnce(&mut InstallJob)) {
    if let Ok(mut map) = jobs().lock() {
        if let Some(j) = map.get_mut(tool) {
            f(j);
        }
    }
}

/// Apply one `--json` progress line to the job.
pub fn apply_event(job: &mut InstallJob, line: &str) {
    let Ok(ev) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if ev.get("type").and_then(Value::as_str) == Some("error") {
        job.error = ev.get("code").and_then(Value::as_str).map(str::to_string);
        job.message = ev.get("message").and_then(Value::as_str).map(str::to_string);
    }
    if ev.get("type").and_then(Value::as_str) == Some("skipped") {
        if let Some(reason) = ev.get("reason").and_then(Value::as_str) {
            if matches!(reason, "terms_required" | "platform") {
                job.error = Some(reason.to_string());
            }
        }
    }
    job.events.push(ev);
    if job.events.len() > MAX_EVENTS {
        let drop = job.events.len() - MAX_EVENTS;
        job.events.drain(..drop);
    }
}

/// Start (or join) the install of one tool. Returns the job snapshot.
pub fn start_install(cmd: InstallerCmd, tool: &str, accept_terms: bool) -> Result<InstallJob, String> {
    if !valid_tool_id(tool) {
        return Err("invalid_tool_id".into());
    }
    {
        let mut map = jobs().lock().map_err(|_| "lock_poisoned".to_string())?;
        if let Some(existing) = map.get(tool) {
            if existing.state == "running" {
                return Ok(existing.clone());
            }
        }
        map.insert(
            tool.to_string(),
            InstallJob {
                tool: tool.to_string(),
                state: "running".into(),
                started_at: now(),
                finished_at: None,
                exit_code: None,
                error: None,
                message: None,
                events: vec![],
            },
        );
    }

    let mut args = cmd.args.clone();
    args.extend(["install", "--only", tool, "--json", "--select"].map(String::from));
    if accept_terms {
        args.extend(["--accept-terms".to_string(), tool.to_string()]);
    }
    let mut command = tokio::process::Command::new(&cmd.program);
    command
        .args(&args)
        .envs(cmd.env.iter().cloned())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(false);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("could not start installer: {e}");
            update(tool, |j| {
                j.state = "failed".into();
                j.finished_at = Some(now());
                j.error = Some("spawn_failed".into());
                j.message = Some(msg.clone());
            });
            return Err(msg);
        }
    };

    let tool_owned = tool.to_string();
    tokio::spawn(async move {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let t_err = tool_owned.clone();
        let err_task = tokio::spawn(async move {
            let mut tail = String::new();
            if let Some(se) = stderr {
                let mut lines = BufReader::new(se).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tail.push_str(&l);
                    tail.push('\n');
                    if tail.len() > 4000 {
                        tail = tail.split_off(tail.len() - 2000);
                    }
                }
            }
            let _ = t_err;
            tail
        });
        if let Some(so) = stdout {
            let mut lines = BufReader::new(so).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                update(&tool_owned, |j| apply_event(j, &l));
            }
        }
        // Installs can be long (npm, pip, downloads); the installer has its
        // own per-step timeouts, this is only a backstop.
        let status = tokio::time::timeout(Duration::from_secs(45 * 60), child.wait()).await;
        let stderr_tail = err_task.await.unwrap_or_default();
        let code = match status {
            Ok(Ok(s)) => s.code(),
            _ => {
                let _ = child.kill().await;
                None
            }
        };
        update(&tool_owned, |j| {
            j.exit_code = code;
            j.finished_at = Some(now());
            let done_ok = j
                .events
                .iter()
                .rev()
                .find(|e| e.get("type").and_then(Value::as_str) == Some("done"))
                .and_then(|e| e.get("ok").and_then(Value::as_bool))
                .unwrap_or(false);
            let installed = j
                .events
                .iter()
                .any(|e| matches!(e.get("type").and_then(Value::as_str), Some("verified")) || (e.get("type").and_then(Value::as_str) == Some("skipped") && e.get("reason").and_then(Value::as_str) == Some("already_installed")));
            if code == Some(0) && done_ok && installed && j.error.is_none() {
                j.state = "succeeded".into();
            } else {
                j.state = "failed".into();
                if j.error.is_none() {
                    j.error = Some(if code.is_none() { "timeout".into() } else { "installer_failed".into() });
                    let t = stderr_tail.trim();
                    if !t.is_empty() {
                        j.message = Some(t.chars().rev().take(600).collect::<String>().chars().rev().collect());
                    }
                }
            }
        });
    });

    job(tool).ok_or_else(|| "job_missing".into())
}

/// `allternit-tools status --json` (manifest + per-tool installed state).
pub async fn list_status(only: Option<&str>) -> Result<Value, String> {
    let cmd = installer_command().ok_or_else(|| "installer_unavailable".to_string())?;
    let mut args = cmd.args.clone();
    args.extend(["status", "--json"].map(String::from));
    if let Some(t) = only {
        if !valid_tool_id(t) {
            return Err("invalid_tool_id".into());
        }
        args.extend(["--only".to_string(), t.to_string()]);
    }
    let fut = tokio::process::Command::new(&cmd.program)
        .args(&args)
        .envs(cmd.env.iter().cloned())
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(Duration::from_secs(90), fut)
        .await
        .map_err(|_| "status_timeout".to_string())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("bad installer output: {e}"))
}

/// Install hint attached to `connect` when the CLI is missing.
pub fn install_action(provider_id: &str) -> Value {
    json!({
        "available": installer_command().is_some(),
        "method": "POST",
        "route": format!("/api/v1/providers/{provider_id}/install"),
        "status_route": format!("/api/v1/providers/{provider_id}/install/status"),
    })
}

#[cfg(test)]
mod provider_tools_install_tests {
    use super::*;

    #[test]
    fn provider_install_rejects_flag_like_or_odd_ids() {
        assert!(valid_tool_id("claude"));
        assert!(valid_tool_id("cursor-agent"));
        assert!(valid_tool_id("gemini-cli"));
        assert!(!valid_tool_id("--prefix"));
        assert!(!valid_tool_id("-x"));
        assert!(!valid_tool_id("a b"));
        assert!(!valid_tool_id("../etc"));
        assert!(!valid_tool_id("Claude"));
        assert!(!valid_tool_id(""));
    }

    fn blank(tool: &str) -> InstallJob {
        InstallJob {
            tool: tool.into(),
            state: "running".into(),
            started_at: now(),
            finished_at: None,
            exit_code: None,
            error: None,
            message: None,
            events: vec![],
        }
    }

    #[test]
    fn provider_install_events_record_errors_and_terms() {
        let mut j = blank("droid");
        apply_event(&mut j, r#"{"type":"start","tool":"droid"}"#);
        apply_event(&mut j, "not json");
        apply_event(&mut j, r#"{"type":"skipped","tool":"droid","reason":"terms_required","url":"https://x"}"#);
        assert_eq!(j.events.len(), 2);
        assert_eq!(j.error.as_deref(), Some("terms_required"));
        let mut k = blank("codex");
        apply_event(&mut k, r#"{"type":"error","tool":"codex","code":"npm_failed","message":"npm exited 1"}"#);
        assert_eq!(k.error.as_deref(), Some("npm_failed"));
        assert_eq!(k.message.as_deref(), Some("npm exited 1"));
    }

    #[test]
    fn provider_install_caps_event_history() {
        let mut j = blank("x");
        for i in 0..(MAX_EVENTS + 50) {
            apply_event(&mut j, &format!(r#"{{"type":"log","line":"{i}"}}"#));
        }
        assert_eq!(j.events.len(), MAX_EVENTS);
    }

    async fn wait_done(tool: &str) -> InstallJob {
        for _ in 0..200 {
            let j = job(tool).unwrap();
            if j.state != "running" {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("install job never finished");
    }

    fn fake(script: &str) -> InstallerCmd {
        InstallerCmd { program: PathBuf::from("/bin/sh"), args: vec!["-c".into(), script.into(), "sh".into()], env: vec![] }
    }

    #[tokio::test]
    async fn provider_install_job_succeeds_on_verified_done() {
        // `sh -c script sh install --only <id> ...` — $3 is the tool id.
        let cmd = fake(r#"echo '{"type":"start","tool":"'$3'"}'; echo '{"type":"verified","tool":"'$3'","version":"1.0.0"}'; echo '{"type":"done","ok":true}'"#);
        let started = start_install(cmd, "fake-ok", false).unwrap();
        assert_eq!(started.state, "running");
        let done = wait_done("fake-ok").await;
        assert_eq!(done.state, "succeeded", "{done:?}");
        assert_eq!(done.exit_code, Some(0));
        assert_eq!(done.events.len(), 3);
    }

    #[tokio::test]
    async fn provider_install_job_fails_with_installer_error_code() {
        let cmd = fake(r#"echo '{"type":"error","tool":"x","code":"checksum_mismatch","message":"sha256 mismatch"}'; echo '{"type":"done","ok":false}'; exit 1"#);
        start_install(cmd, "fake-bad", false).unwrap();
        let done = wait_done("fake-bad").await;
        assert_eq!(done.state, "failed");
        assert_eq!(done.error.as_deref(), Some("checksum_mismatch"));
    }

    #[tokio::test]
    async fn provider_install_terms_skip_is_not_success() {
        let cmd = fake(r#"echo '{"type":"skipped","tool":"x","reason":"terms_required"}'; echo '{"type":"done","ok":true}'"#);
        start_install(cmd, "fake-terms", false).unwrap();
        let done = wait_done("fake-terms").await;
        assert_eq!(done.state, "failed");
        assert_eq!(done.error.as_deref(), Some("terms_required"));
    }

    #[tokio::test]
    async fn provider_install_passes_accept_terms_for_that_tool_only() {
        let cmd = fake(r#"echo "{\"type\":\"log\",\"line\":\"$*\"}"; echo '{"type":"verified"}'; echo '{"type":"done","ok":true}'"#);
        start_install(cmd, "fake-args", true).unwrap();
        let done = wait_done("fake-args").await;
        let line = done.events[0]["line"].as_str().unwrap().to_string();
        assert_eq!(line, "install --only fake-args --json --select --accept-terms fake-args");
    }
}
