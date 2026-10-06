//! `ao visibility` — headless JSON dump of the P5 "who needs you" panel.
//!
//! This is the panel's CLI twin (spec binding decision 2: "ao agent list JSON
//! is the headless contract" — this command reuses that contract for feed 1
//! and adds feeds 2 and 3). It exists so the merged panel is scriptable and
//! demo-verifiable without driving the TUI.

use std::io;

use crate::ao::native::{home_dir, list_harnesses, list_native_sessions};
use crate::ao::peers;
use crate::ao::visibility::{build_panel, now_ms, FeedSample, WaitingList};
use crate::api::schema::AgentInfo;
use crate::api::schema::{EmptyParams, Method, Request};

pub fn run_visibility_command(args: &[String]) -> io::Result<i32> {
    let mut root_flag: Option<String> = None;
    let mut cwd_filter: Option<String> = None;
    let mut home_flag: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => root_flag = iter.next().cloned(),
            "--cwd" => cwd_filter = iter.next().cloned(),
            "--home" => home_flag = iter.next().cloned(),
            "help" | "--help" | "-h" => {
                eprintln!(
                    "usage: ao visibility [--root <dir>] [--cwd <dir>] [--home <dir>]\n\n\
                     Merged P5 panel snapshot as JSON: engine agents (agent.list),\n\
                     waiting-on-you list, native CLI sessions (catalog), Rails peers."
                );
                return Ok(0);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    // Feed 1: engine agents via the existing socket contract. A missing
    // server degrades to an empty engine section with the error noted.
    let engine = engine_feed();

    // Feed 2: native catalog (home override for tests; cwd filter optional).
    let home = home_flag.map(std::path::PathBuf::from).or_else(home_dir);
    let (native, harnesses) = match &home {
        Some(home) => (
            list_native_sessions(home, None, cwd_filter.as_deref()),
            list_harnesses(home),
        ),
        None => (Vec::new(), Vec::new()),
    };

    // Feed 3: Rails peers under the resolved root.
    let peers_root = peers::registry_root(root_flag.as_deref())?;
    let peer_rows = peers::read_registry(&peers_root).unwrap_or_default();

    let sample = FeedSample {
        engine,
        native,
        peers: peer_rows,
    };

    // Headless: no poll history, so the waiting list is rebuilt from the
    // current blocked set (transitions are a client-runtime concept).
    let mut waiting = WaitingList::new();
    let now = now_ms();
    if let Ok(agents) = sample.engine.as_ref() {
        waiting.rebuild(agents, now);
    }

    let panel = build_panel(sample, &waiting, harnesses, now);
    println!("{}", serde_json::to_string_pretty(&panel).unwrap());
    Ok(0)
}

/// `agent.list` over the local socket; `Err(reason)` when no server answers.
fn engine_feed() -> Result<Vec<AgentInfo>, String> {
    let response = super::send_request_unchecked(&Request {
        id: "cli:visibility".into(),
        method: Method::AgentList(EmptyParams::default()),
    })
    .map_err(|err| format!("engine agent.list failed: {err}"))?;
    crate::ao::visibility::parse_agent_list(&response)
}
