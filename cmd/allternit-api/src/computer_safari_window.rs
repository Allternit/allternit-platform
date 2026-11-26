//! One Safari window per agent browser run on this device (macOS), the API
//! half of the Safari driver lane (platform #3122).
//!
//! A browser run (`POST /api/aci/run` or `POST /api/v1/computers/:id/runs`
//! aimed at this device) is marked at start ([`mark_browser_run`]). On the
//! run's first computer-toolset call on this device, [`ensure`] asks the
//! local driver for `safari_window {op: "open"}`: a new window in the
//! "Allternit Agent" Safari profile, answered as `{pid, window_id}` (a
//! WindowTarget). The run keeps it; a `read_ui` of the run that names no
//! window reads that window ([`scope_read_ui`]), so the agent's element ids
//! (and the `act` calls on them) belong to its own window, never the
//! person's.
//!
//! The run ends ([`end_run`], called from `computer_agent_display::end_run`,
//! so the same places as the Linux display: the ACI run finishing, a stop, a
//! stop or re-run at a blocked page) or after [`IDLE_STOP`] without a call;
//! the window is then closed with `safari_window {op: "close", pid,
//! window_id}`. The driver only closes windows it opened.
//!
//! When Safari has no agent profile and the driver can't make one by script,
//! it answers `profile_missing` with `data.steps`. The run then holds the way
//! `request_human` does: `computer.human_requested` with reason
//! [`PROFILE_REASON`] and the `steps`, the `aci_human` push, and a wait for
//! `POST /computers/:id/human-done`. Desktop (#3116) and the phones show the
//! card. After the hold the open is tried once more; still missing, the call
//! fails with `safari_profile_missing`, and the run does not drive the
//! person's own Safari windows. Any other refusal (no arc engine, no
//! sidecar) leaves the run as it was before this lane: no window of its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::auth::AuthUser;
use crate::computer_routes::ComputerResponse;
use crate::computer_toolset::{Fail, Target};
use crate::this_device_input::{self as td, DriverReply};
use crate::AppState;

/// A run with no toolset call for this long closes its window.
pub const IDLE_STOP: Duration = Duration::from_secs(15 * 60);
/// How long the setup card waits for the person.
pub const PROFILE_HOLD_MS: u64 = 600_000;
/// The setup card's reason (`computer.human_requested`).
pub const PROFILE_REASON: &str = "Set up the Allternit Agent Safari profile";
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
/// A marked run that never makes a call is forgotten after this long.
const MARK_TTL: Duration = Duration::from_secs(6 * 3600);

/// One run's Safari window (the driver's WindowTarget).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWindow {
    pub computer_id: String,
    pub pid: i64,
    pub window_id: i64,
}

struct Entry {
    window: RunWindow,
    last_used: Instant,
}

/// Runs started as agent browser runs, not yet given a window.
static BROWSER_RUNS: Lazy<Mutex<HashMap<String, Instant>>> = Lazy::new(|| Mutex::new(HashMap::new()));
/// Runs that have their window.
static WINDOWS: Lazy<Mutex<HashMap<String, Entry>>> = Lazy::new(|| Mutex::new(HashMap::new()));
/// One open at a time, so two first calls of one run can't open two windows.
static OPENING: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

/// Record that `run_id` is an agent browser run (its first call on this
/// device opens its window).
pub fn mark_browser_run(run_id: &str) {
    let mut runs = BROWSER_RUNS.lock().unwrap_or_else(|p| p.into_inner());
    runs.retain(|_, at| at.elapsed() < MARK_TTL);
    runs.insert(run_id.to_string(), Instant::now());
}

fn is_browser_run(run_id: &str) -> bool {
    BROWSER_RUNS.lock().unwrap_or_else(|p| p.into_inner()).contains_key(run_id)
}

/// The run's window, when it has one (and a toolset call just used it).
pub fn window_of(run_id: &str) -> Option<RunWindow> {
    let mut windows = WINDOWS.lock().unwrap_or_else(|p| p.into_inner());
    windows.get_mut(run_id).map(|e| {
        e.last_used = Instant::now();
        e.window.clone()
    })
}

/// What a `safari_window open` answer means for the run.
#[derive(Debug, Clone, PartialEq)]
pub enum OpenOutcome {
    Opened { pid: i64, window_id: i64 },
    /// No agent profile and the driver couldn't make one: hold for the person.
    ProfileMissing { reason: Option<String>, steps: Vec<String> },
    /// Any other refusal: the run goes on without a window of its own.
    Unavailable(String),
}

/// Read one driver answer to `safari_window {op: "open"}`.
pub fn read_open(reply: &DriverReply) -> OpenOutcome {
    match reply {
        DriverReply::Ok(v) => match (v.get("pid").and_then(Value::as_i64), v.get("window_id").and_then(Value::as_i64)) {
            (Some(pid), Some(window_id)) if pid > 0 && window_id > 0 => OpenOutcome::Opened { pid, window_id },
            _ => OpenOutcome::Unavailable("the driver answered without a window".into()),
        },
        DriverReply::Refused { code: Some(code), data, .. } if code == "profile_missing" => OpenOutcome::ProfileMissing {
            reason: data.get("reason").and_then(Value::as_str).map(str::to_string),
            steps: data
                .get("steps")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.chars().take(300).collect::<String>()).take(12).collect())
                .unwrap_or_default(),
        },
        DriverReply::Refused { code, message, .. } => {
            OpenOutcome::Unavailable(format!("{}: {}", code.as_deref().unwrap_or("error"), message.chars().take(200).collect::<String>()))
        }
    }
}

/// The `computer.human_requested` fields a `profile_missing` answer adds:
/// `kind: "safari_profile"`, the driver's `steps`, and its `reason`.
pub fn profile_hold_extra(reason: Option<&str>, steps: &[String]) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), json!("safari_profile"));
    m.insert("steps".into(), json!(steps));
    if let Some(r) = reason {
        m.insert("setup_reason".into(), json!(r));
    }
    m
}

async fn open_once() -> OpenOutcome {
    let Some(endpoint) = td::DriverEndpoint::resolve() else {
        return OpenOutcome::Unavailable("the driver sidecar isn't running".into());
    };
    match td::driver_rpc_reply(&endpoint, "safari_window", json!({ "op": "open" }), OPEN_TIMEOUT).await {
        Ok(reply) => read_open(&reply),
        Err(e) => OpenOutcome::Unavailable(String::from(e)),
    }
}

/// Give a browser run on this device its own Safari window on its first
/// call. `Ok` = go on (with or without a window); `Err` = the agent profile
/// is still missing after the person's setup window.
pub async fn ensure(state: &Arc<AppState>, user: &AuthUser, computer: &ComputerResponse, target: &Target, run_id: &str) -> Result<(), Fail> {
    if !cfg!(target_os = "macos") || !matches!(target, Target::ThisDevice) {
        return Ok(());
    }
    // Every call of a run with a window counts as use (the idle close).
    if window_of(run_id).is_some() || !is_browser_run(run_id) {
        return Ok(());
    }
    let mut held = false;
    loop {
        let outcome = {
            let _one = OPENING.lock().await;
            if window_of(run_id).is_some() {
                return Ok(());
            }
            let outcome = open_once().await;
            if let OpenOutcome::Opened { pid, window_id } = outcome {
                WINDOWS.lock().unwrap_or_else(|p| p.into_inner()).insert(
                    run_id.to_string(),
                    Entry { window: RunWindow { computer_id: computer.id.clone(), pid, window_id }, last_used: Instant::now() },
                );
                BROWSER_RUNS.lock().unwrap_or_else(|p| p.into_inner()).remove(run_id);
                spawn_idle_watch(run_id.to_string());
                return Ok(());
            }
            outcome
        };
        match outcome {
            OpenOutcome::Opened { .. } => return Ok(()),
            OpenOutcome::Unavailable(why) => {
                warn!(run_id, %why, "no Safari agent window; the run goes on without one");
                BROWSER_RUNS.lock().unwrap_or_else(|p| p.into_inner()).remove(run_id);
                return Ok(());
            }
            OpenOutcome::ProfileMissing { .. } if held => {
                return Err(Fail::coded(
                    "safari_profile_missing",
                    "Safari has no \"Allternit Agent\" profile, so this run has no Safari window of its own and won't drive the person's windows. Stop here and tell the person to set up the profile (Safari > Settings > Profiles), then start the task again.",
                ));
            }
            OpenOutcome::ProfileMissing { reason, steps } => {
                held = true;
                crate::computer_v2::hold_for_person(
                    state,
                    user,
                    computer,
                    target,
                    run_id,
                    PROFILE_REASON,
                    profile_hold_extra(reason.as_deref(), &steps),
                    PROFILE_HOLD_MS,
                )
                .await?;
            }
        }
    }
}

/// A `read_ui` of a run that has its window and names no window reads that
/// window. Anything else passes through unchanged.
pub fn scope_read_ui(member: &str, input: &Value, window: Option<&RunWindow>) -> Option<Value> {
    let window = window?;
    if member != "read_ui" || ["app", "pid", "window_id"].iter().any(|k| input.get(*k).is_some_and(|v| !v.is_null())) {
        return None;
    }
    let mut scoped = input.as_object().cloned().unwrap_or_default();
    scoped.insert("pid".into(), json!(window.pid));
    scoped.insert("window_id".into(), json!(window.window_id));
    Some(Value::Object(scoped))
}

/// The run ended: close its window (and forget the mark if it never got one).
pub fn end_run(run_id: &str) {
    BROWSER_RUNS.lock().unwrap_or_else(|p| p.into_inner()).remove(run_id);
    let Some(entry) = WINDOWS.lock().unwrap_or_else(|p| p.into_inner()).remove(run_id) else { return };
    let run = run_id.to_string();
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    rt.spawn(async move {
        let Some(endpoint) = td::DriverEndpoint::resolve() else { return };
        let params = json!({ "op": "close", "pid": entry.window.pid, "window_id": entry.window.window_id });
        match td::driver_rpc_reply(&endpoint, "safari_window", params, CLOSE_TIMEOUT).await {
            Ok(DriverReply::Ok(_)) => {}
            Ok(DriverReply::Refused { code, message, .. }) => warn!(run_id = %run, ?code, %message, "closing the run's Safari window was refused"),
            Err(e) => warn!(run_id = %run, error = ?e, "closing the run's Safari window failed"),
        }
    });
}

/// Close a run's window after `IDLE_STOP` without a call.
fn spawn_idle_watch(run_id: String) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let idle = match WINDOWS.lock().unwrap_or_else(|p| p.into_inner()).get(&run_id) {
                Some(e) => e.last_used.elapsed() >= IDLE_STOP,
                None => return,
            };
            if idle {
                crate::computer_agent_display::end_run(&run_id);
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_missing_becomes_a_setup_hold_with_the_driver_steps() {
        // The driver's JSON-RPC refusal, as server.py sends it.
        let envelope = json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32000, "message": "Safari has no Allternit Agent profile",
                       "data": { "code": "profile_missing", "reason": "no_script_access", "profile": "Allternit Agent",
                                 "steps": ["Open Safari > Settings > Profiles.", "Click + and name it Allternit Agent."] } }
        });
        let outcome = read_open(&DriverReply::from_envelope(&envelope));
        let OpenOutcome::ProfileMissing { reason, steps } = outcome else { panic!("{outcome:?}") };
        assert_eq!(reason.as_deref(), Some("no_script_access"));
        assert_eq!(steps.len(), 2);

        let extra = profile_hold_extra(reason.as_deref(), &steps);
        assert_eq!(extra["kind"], "safari_profile");
        assert_eq!(extra["steps"][1], "Click + and name it Allternit Agent.");
        assert_eq!(PROFILE_REASON, "Set up the Allternit Agent Safari profile");

        // An opened window and other refusals.
        let ok = DriverReply::from_envelope(&json!({ "result": { "pid": 412, "window_id": 9001 } }));
        assert_eq!(read_open(&ok), OpenOutcome::Opened { pid: 412, window_id: 9001 });
        let no_arc = DriverReply::from_envelope(&json!({ "error": { "message": "Safari agent windows need the arc engine (macOS)", "data": { "code": "engine_unavailable" } } }));
        assert!(matches!(read_open(&no_arc), OpenOutcome::Unavailable(_)));
    }

    #[test]
    fn read_ui_without_a_window_reads_the_runs_window() {
        let w = RunWindow { computer_id: "c1".into(), pid: 412, window_id: 9001 };
        let scoped = scope_read_ui("read_ui", &json!({ "full": true }), Some(&w)).unwrap();
        assert_eq!((scoped["pid"].as_i64(), scoped["window_id"].as_i64(), scoped["full"].as_bool()), (Some(412), Some(9001), Some(true)));
        assert!(scope_read_ui("read_ui", &json!({ "app": "Notes" }), Some(&w)).is_none(), "a named window wins");
        assert!(scope_read_ui("act", &json!({ "id": "e1" }), Some(&w)).is_none());
        assert!(scope_read_ui("read_ui", &json!({}), None).is_none());
    }
}
