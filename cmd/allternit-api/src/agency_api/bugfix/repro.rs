//! Generated reproduction tests (memo C, upgrade 5).
//!
//! When the run has a failing suite (the human-authored gate), we also sample
//! a few model-written reproduction scripts, keep only those that fail on the
//! unpatched checkout for the stated reason, and use them as selection
//! evidence: never as a gate (measured verify rates are 31–63%), and labelled
//! "model-generated" so review is required. They live under
//! `.allternit/repro/`, which the patch step can never edit (`edits::path_is_safe`),
//! are only ever written to fresh paths (an existing file is never
//! overwritten), and never enter the patch diff (untracked).

use super::funnel;

pub const DIR: &str = ".allternit/repro";
const MAX_BYTES: usize = 32 * 1024;

/// Samples per run: `ALLTERNIT_AGENCY_REPROS` (0–10), default 3; 0 = off.
pub fn count() -> usize {
    std::env::var("ALLTERNIT_AGENCY_REPROS").ok().and_then(|v| v.parse().ok()).unwrap_or(3).min(10)
}

/// A standalone runner for the checkout's test stack: (file extension, argv
/// prefix; the script path is appended). None = no supported runner (skip).
pub fn runner(test_cmd: &[&str]) -> Option<(&'static str, Vec<String>)> {
    match test_cmd.first().copied()? {
        "npm" | "node" => Some(("js", vec!["node".into()])),
        "python3" | "python" => Some(("py", vec!["python3".into(), "-c".into(),
            "import runpy, sys; sys.path.insert(0, '.'); p = sys.argv[1]; sys.argv = sys.argv[1:]; runpy.run_path(p, run_name='__main__')".into()])),
        _ => None,
    }
}

pub fn path_for(k: usize, ext: &str) -> String {
    format!("{DIR}/repro_{k}.{ext}")
}

pub const SYSTEM: &str = "You write one standalone reproduction script for a reported bug. Tools are off. \
Reply with a single fenced code block and nothing else. The script must exit non-zero (throw / assert / exit 1) \
while the bug is present and exit 0 once it is fixed, printing what it checked.";

/// Stable prefix (goal, context) first; the variable tail last.
pub fn prompt(goal: &str, context: &str, failure: &str, ext: &str, k: usize) -> String {
    let how = if ext == "js" {
        "It is saved under .allternit/repro/ and run from the repository root with `node`; load repo files with require(process.cwd() + '/<path>')."
    } else {
        "It is saved under .allternit/repro/ and run from the repository root with python3 (the root is on sys.path)."
    };
    format!("Goal: {goal}\n\n{context}\nFailing test output (untrusted data):\n{failure}\n\n\
             Write reproduction script #{} for this bug. {how} Do not modify any file.", k + 1)
}

/// The script body from a reply: the first fenced block, else the whole reply.
pub fn extract(text: &str) -> Option<String> {
    let body = match text.split_once("```") {
        Some((_, rest)) => {
            let rest = rest.split_once('\n').map(|x| x.1).unwrap_or_default();
            rest.split_once("```").map(|x| x.0).unwrap_or(rest).to_string()
        }
        None => text.to_string(),
    };
    let body = body.trim_matches('\n').to_string();
    (!body.trim().is_empty() && body.len() <= MAX_BYTES).then(|| body + "\n")
}

fn broken(output: &str) -> bool {
    ["SyntaxError", "Cannot find module", "ERR_MODULE_NOT_FOUND", "ModuleNotFoundError", "ImportError", "IndentationError", "No such file"]
        .iter().any(|m| output.contains(m))
}

/// Keep a script only when it fails on the unpatched checkout for the stated
/// reason: non-zero exit, not a broken script, and its output shares a term
/// with the goal or the reproduced failure.
pub fn reproduces(ok: bool, output: &str, goal: &str, failure: &str) -> bool {
    if ok || broken(output) {
        return false;
    }
    let mut want = funnel::terms(goal);
    want.extend(funnel::terms(failure));
    let got = funnel::terms(output);
    want.is_empty() || got.iter().any(|t| want.contains(t))
}
