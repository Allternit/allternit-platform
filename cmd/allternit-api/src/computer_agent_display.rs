//! One X display + headed Chrome per agent run on Linux guests (the API half
//! of the agent-browser Linux lane, platform #3113).
//!
//! The guest image ships `allternit-agent-display` (domains/computer-use/
//! driver/allternit_driver/agent_display.py). On the first computer-toolset
//! call of a run on a `guest_linux` target, [`ensure`] runs
//! `allternit-agent-display start <agent_id>` through the guest exec channel
//! and keeps `{display, pid, cdp_port, vnc_port}` for the run. From then on
//! the run's `Target::Guest` carries `agent_display`, so the guest driver's
//! `read_ui` / `act` / `run_batch` / `verify` / `screenshot` / `pixel_*`
//! calls and `capture()` all name that display (`guest_driver::guest_driver_raw`
//! adds it), and the xdotool fallback exports it as `DISPLAY`.
//!
//! The run ends ([`end_run`]) when its ACI run finishes or is stopped, when
//! the person stops it at a blocked page, or after [`IDLE_STOP`] without a
//! call. The display stops only when the last run of that agent on that
//! computer ends (two runs of one bot share its display and profile, the way
//! `start` reuses a live display). `stop` keeps the Chrome profile, so logins
//! carry over to the agent's next run.
//!
//! An image without the launcher (exit 127), or one whose launcher fails for
//! another reason (no Chrome, no Xvfb: exit 2), runs on the shared desktop
//! display as before. Only `display_limit` (exit 3: the computer has no
//! memory for another agent browser) fails the run start.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use tracing::warn;

use crate::computer_toolset::Target;

/// A run with no toolset call for this long gives its display back.
pub const IDLE_STOP: Duration = Duration::from_secs(15 * 60);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(20);
/// The launcher's exit code when the computer has no room for another display.
const EXIT_DISPLAY_LIMIT: i32 = 3;
/// `command -v` failed: the image predates the launcher.
const EXIT_NO_LAUNCHER: i32 = 127;

/// One run's display, as the launcher reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunDisplay {
    pub computer_id: String,
    pub agent_id: String,
    pub display: String,
    pub pid: Option<u64>,
    pub cdp_port: Option<u64>,
    pub vnc_port: Option<u64>,
}

struct Entry {
    info: RunDisplay,
    last_used: Instant,
    driver: Arc<dyn allternit_driver_interface::ExecutionDriver>,
    handle: allternit_driver_interface::ExecutionHandle,
}

static RUNS: Lazy<Mutex<HashMap<String, Entry>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Why a run can't start on its own display.
#[derive(Debug, PartialEq)]
pub enum StartError {
    /// Exit 3: no memory for another agent browser on this computer.
    DisplayLimit(String),
}

impl StartError {
    pub fn message(&self) -> String {
        match self {
            StartError::DisplayLimit(detail) => format!(
                "This computer has no room for another agent browser ({detail}). Stop another agent's run on it, or use a computer with more memory."
            ),
        }
    }
}

/// The launcher's agent id: the bot/agent id when it fits the launcher's
/// rule (`[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}`), else one derived from the run
/// id, so the profile is per agent when the caller names one.
pub fn launcher_agent_id(agent_id: Option<&str>, run_id: &str) -> String {
    let fits = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    if let Some(a) = agent_id.map(str::trim).filter(|a| fits(a)) {
        return a.to_string();
    }
    let cleaned: String = run_id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).take(56).collect();
    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        format!("run-{cleaned}")
    }
}

/// What a launcher answer means for the run.
#[derive(Debug, PartialEq)]
pub enum LaunchOutcome {
    Ready(RunDisplay),
    /// Run on the shared desktop display (no launcher, or it failed softly).
    Shared(String),
    Refused(StartError),
}

/// Read one `allternit-agent-display start` answer (exit code + stdout).
pub fn read_launch(computer_id: &str, agent_id: &str, exit_code: i32, stdout: &str) -> LaunchOutcome {
    let last = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or_default();
    let body: Value = serde_json::from_str(last).unwrap_or(Value::Null);
    match exit_code {
        0 => match body.get("display").and_then(Value::as_str).filter(|d| display_ok(d)) {
            Some(display) => LaunchOutcome::Ready(RunDisplay {
                computer_id: computer_id.to_string(),
                agent_id: agent_id.to_string(),
                display: display.to_string(),
                pid: body.get("pid").and_then(Value::as_u64),
                cdp_port: body.get("cdp_port").and_then(Value::as_u64),
                vnc_port: body.get("vnc_port").and_then(Value::as_u64),
            }),
            None => LaunchOutcome::Shared("the launcher answered without a display".into()),
        },
        EXIT_DISPLAY_LIMIT => LaunchOutcome::Refused(StartError::DisplayLimit(
            body.get("message").and_then(Value::as_str).unwrap_or("display_limit").chars().take(300).collect(),
        )),
        EXIT_NO_LAUNCHER => LaunchOutcome::Shared("this computer's image has no allternit-agent-display".into()),
        other => LaunchOutcome::Shared(format!(
            "allternit-agent-display failed (exit {other}): {}",
            body.get("message").and_then(Value::as_str).unwrap_or(last).chars().take(200).collect::<String>()
        )),
    }
}

/// `:N` with N a number: the only shape passed on as a display.
pub fn display_ok(d: &str) -> bool {
    d.strip_prefix(':').is_some_and(|n| !n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit()))
}

async fn exec(
    driver: &Arc<dyn allternit_driver_interface::ExecutionDriver>,
    handle: &allternit_driver_interface::ExecutionHandle,
    script: String,
    timeout: Duration,
) -> Result<(i32, String), String> {
    let spec = allternit_driver_interface::CommandSpec {
        command: vec!["sh".into(), "-c".into(), script],
        env_vars: HashMap::new(),
        working_dir: None,
        stdin_data: None,
        capture_stdout: true,
        capture_stderr: true,
    };
    let out = tokio::time::timeout(timeout, driver.exec(handle, spec))
        .await
        .map_err(|_| "the computer didn't answer in time".to_string())?
        .map_err(|e| format!("couldn't reach the computer: {e}"))?;
    Ok((out.exit_code, String::from_utf8_lossy(out.stdout.as_deref().unwrap_or(&[])).to_string()))
}

/// The run's own display on a Linux guest, starting it on the run's first
/// call. `Ok(None)` = run on the shared display (not a Linux guest, or the
/// image can't start one). `Err` = the computer refused (display_limit).
pub async fn ensure(target: &Target, computer_id: &str, run_id: &str, agent_id: Option<&str>) -> Result<Option<String>, StartError> {
    let Target::Guest { driver, handle, os, .. } = target else { return Ok(None) };
    if os == "windows" {
        return Ok(None);
    }
    if let Some(e) = RUNS.lock().unwrap_or_else(|p| p.into_inner()).get_mut(run_id) {
        e.last_used = Instant::now();
        return Ok(Some(e.info.display.clone()));
    }
    let agent = launcher_agent_id(agent_id, run_id);
    // The id is checked against the launcher's own rule above, so it is
    // safe unquoted; quoted anyway.
    let script = format!(
        "command -v allternit-agent-display >/dev/null 2>&1 || exit {EXIT_NO_LAUNCHER}; allternit-agent-display start '{agent}'"
    );
    let (code, stdout) = match exec(driver, handle, script, START_TIMEOUT).await {
        Ok(r) => r,
        Err(e) => {
            warn!(run_id, error = %e, "agent display start didn't answer; the run uses the shared display");
            return Ok(None);
        }
    };
    match read_launch(computer_id, &agent, code, &stdout) {
        LaunchOutcome::Ready(info) => {
            let display = info.display.clone();
            emit(&info, run_id, "started");
            RUNS.lock().unwrap_or_else(|p| p.into_inner()).insert(
                run_id.to_string(),
                Entry { info, last_used: Instant::now(), driver: driver.clone(), handle: handle.clone() },
            );
            spawn_idle_watch(run_id.to_string());
            Ok(Some(display))
        }
        LaunchOutcome::Shared(why) => {
            warn!(run_id, %why, "the run uses the shared display");
            Ok(None)
        }
        LaunchOutcome::Refused(e) => Err(e),
    }
}

/// The display a run uses, when it has its own.
pub fn display_of(run_id: &str) -> Option<RunDisplay> {
    RUNS.lock().unwrap_or_else(|p| p.into_inner()).get(run_id).map(|e| e.info.clone())
}

/// The display the computer's live view shows: the given run's display, or,
/// with no run named, the one run on that computer that has its own display
/// (several runs at once: `None`, the caller names one).
pub fn live_view_display(computer_id: &str, run_id: Option<&str>) -> Option<String> {
    let runs = RUNS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(run) = run_id {
        return runs.get(run).filter(|e| e.info.computer_id == computer_id).map(|e| e.info.display.clone());
    }
    let mut on_computer = runs.values().filter(|e| e.info.computer_id == computer_id);
    match (on_computer.next(), on_computer.next()) {
        (Some(only), None) => Some(only.info.display.clone()),
        _ => None,
    }
}

/// The run ended: forget what it kept (block continue marks, run-scoped
/// credential approvals) and stop its display when it was the last run of
/// that agent on that computer.
pub fn end_run(run_id: &str) {
    crate::computer_block::forget_run(run_id);
    let ended = RUNS.lock().unwrap_or_else(|p| p.into_inner()).remove(run_id);
    let Some(entry) = ended else { return };
    let shared = RUNS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .values()
        .any(|e| e.info.computer_id == entry.info.computer_id && e.info.agent_id == entry.info.agent_id);
    if shared {
        return;
    }
    let run = run_id.to_string();
    tokio::spawn(async move {
        let script = format!("allternit-agent-display stop '{}'", entry.info.agent_id);
        match exec(&entry.driver, &entry.handle, script, STOP_TIMEOUT).await {
            Ok((0, _)) => emit(&entry.info, &run, "stopped"),
            Ok((code, out)) => warn!(run_id = %run, code, out = %out.trim(), "agent display stop failed"),
            Err(e) => warn!(run_id = %run, error = %e, "agent display stop didn't answer"),
        }
    });
}

/// Stop a run's display after `IDLE_STOP` without a call (a run that never
/// says it ended).
fn spawn_idle_watch(run_id: String) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let idle = match RUNS.lock().unwrap_or_else(|p| p.into_inner()).get(&run_id) {
                Some(e) => e.last_used.elapsed() >= IDLE_STOP,
                None => return,
            };
            if idle {
                end_run(&run_id);
                return;
            }
        }
    });
}

/// `computer.agent_display` on the toolset event stream: clients point the
/// run's live view at `display` (VNC on `vnc_port` on the desktop image; the
/// screenshot route takes `?run_id=`).
fn emit(info: &RunDisplay, run_id: &str, state: &str) {
    let event = json!({
        "type": "computer.agent_display",
        "ts": chrono::Utc::now().to_rfc3339(),
        "data": {
            "run_id": run_id,
            "computer_id": info.computer_id,
            "agent_id": info.agent_id,
            "display": info.display,
            "vnc_port": info.vnc_port,
            "state": state,
        },
    });
    let _ = crate::computer_toolset::ACTION_EVENTS.send((info.computer_id.clone(), event));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ids_follow_the_launcher_rule() {
        assert_eq!(launcher_agent_id(Some("bot_7f3a"), "r1"), "bot_7f3a");
        assert_eq!(launcher_agent_id(Some("../etc"), "run 1;rm"), "run-run1rm");
        assert_eq!(launcher_agent_id(Some("a'b"), "x"), "run-x");
        assert_eq!(launcher_agent_id(None, "!!"), "agent");
    }

    #[test]
    fn launcher_answers_map_to_ready_shared_or_refused() {
        let ok = r#"{"agent_id":"a","display":":10","pid":42,"cdp_port":9310,"vnc_port":5910}"#;
        match read_launch("c1", "a", 0, ok) {
            LaunchOutcome::Ready(d) => {
                assert_eq!(d.display, ":10");
                assert_eq!((d.pid, d.cdp_port, d.vnc_port), (Some(42), Some(9310), Some(5910)));
            }
            other => panic!("{other:?}"),
        }
        let limit = r#"{"error":"display_limit","message":"this computer can run 6 agent browser(s)"}"#;
        match read_launch("c1", "a", 3, limit) {
            LaunchOutcome::Refused(e) => assert!(e.message().contains("can run 6"), "{}", e.message()),
            other => panic!("{other:?}"),
        }
        assert!(matches!(read_launch("c1", "a", 127, ""), LaunchOutcome::Shared(_)), "old image: shared display");
        assert!(matches!(read_launch("c1", "a", 2, r#"{"error":"chrome_missing"}"#), LaunchOutcome::Shared(_)));
        assert!(matches!(read_launch("c1", "a", 0, r#"{"display":"; rm -rf /"}"#), LaunchOutcome::Shared(_)), "only :N passes");
    }
}
