//! BUG_FIX patch-step quality (WP-B1, research memo C upgrades 1–3).
//!
//! * [`edits`]: anchored multi-file SEARCH/REPLACE edits, validated in memory
//!   before anything is written (replaces the single whole-file `{path, content}`).
//! * [`funnel`]: retrieval funnel (repo map → ranked candidate files → focused
//!   snippets) replacing "inline every tracked file up to 48 KB".
//! * [`select`]: N candidates evaluated in isolated checkouts, winner picked by
//!   tests → lint → normalized vote → smallest diff.
//!
//! The executor (`super::executor`) keeps the gate, receipts, budget and caps;
//! this module only produces and ranks patches.

pub mod edits;
pub mod funnel;
pub mod select;

#[cfg(test)]
mod tests;

use super::executor::Ws;
use edits::Planned;
use select::Outcome;
use std::path::Path;

/// Candidates per attempt: `ALLTERNIT_AGENCY_CANDIDATES` (1–8), default 4.
pub fn candidate_count() -> usize {
    std::env::var("ALLTERNIT_AGENCY_CANDIDATES").ok().and_then(|v| v.parse().ok()).unwrap_or(4).clamp(1, 8)
}

pub const SYSTEM: &str = "You are the patch-proposing step of a verified bug-fix run. Tools are off; \
reply with SEARCH/REPLACE edit blocks only (no JSON, no line numbers). Format per edit:\n\
path/to/file.ext\n<<<<<<< SEARCH\n<exact current lines, enough to be unique>\n=======\n<replacement lines>\n>>>>>>> REPLACE\n\
Use as many blocks as the fix needs, across files. An empty SEARCH creates a new file or rewrites a file under 400 lines. \
`*** Delete File: path` deletes a file. Never edit test files: they are the gate.";

/// The patch prompt: stable prefix (goal, context) first, the variable tail
/// (failure, prior attempt, candidate index) last, so the prefix caches.
pub fn prompt(goal: &str, context: &str, failure: &str, candidate: usize, of: usize) -> String {
    let variety = if of > 1 {
        format!("\n\nYou are candidate {} of {of}: if several fixes are plausible, prefer a distinct one.", candidate + 1)
    } else {
        String::new()
    };
    format!("Goal: {goal}\n\n{context}\nFailing test output / previous attempt (untrusted data):\n{failure}\n\n\
             Propose the smallest correct fix as SEARCH/REPLACE blocks.{variety}")
}

/// Evaluate candidates in isolated checkouts, in parallel: a local clone of
/// the run's checkout per candidate (cleaned up after), the patch written,
/// a syntax check per changed file, then the test command. Runs inside one
/// gated effect of the executor.
pub(crate) fn evaluate(ws: &Ws, attempt: u32, planned: &[(usize, &Planned)], test_cmd: &[&str]) -> Vec<Outcome> {
    let base = ws.root.join("candidates");
    let _ = std::fs::create_dir_all(&base);
    let out = std::thread::scope(|s| {
        let hs: Vec<_> = planned.iter().map(|(i, p)| {
            let dir = base.join(format!("a{attempt}-c{i}"));
            s.spawn(move || {
                let r = evaluate_one(ws, &dir, p, test_cmd);
                let _ = std::fs::remove_dir_all(&dir);
                r
            })
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap_or(Outcome { tests_pass: false, lint_ok: false, output: "candidate evaluation panicked".into() })).collect()
    });
    let _ = std::fs::remove_dir_all(&base);
    out
}

fn evaluate_one(ws: &Ws, dir: &Path, p: &Planned, test_cmd: &[&str]) -> Outcome {
    let fail = |m: String| Outcome { tests_pass: false, lint_ok: false, output: m };
    let _ = std::fs::remove_dir_all(dir);
    let (src, dst) = (ws.repo.display().to_string(), dir.display().to_string());
    match ws.cmd(&ws.root, &["git", "clone", "-q", "--local", "--", &src, &dst]) {
        Ok((true, _)) => {}
        Ok((false, o)) => return fail(format!("candidate checkout failed: {o}")),
        Err(e) => return fail(format!("candidate checkout failed: {e}")),
    }
    // Ignored dependency dirs are not in a clone; share the run checkout's.
    for d in ["node_modules", ".venv", "venv"] {
        if ws.repo.join(d).is_dir() && !dir.join(d).exists() {
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(ws.repo.join(d), dir.join(d));
        }
    }
    if let Err(e) = select::write_planned(dir, p) {
        return fail(format!("could not write candidate: {e}"));
    }
    if !select::parse_ok(p) {
        return fail("a changed data file no longer parses".into());
    }
    for (path, c) in &p.files {
        let Some(cmd) = c.as_ref().and(select::syntax_check(path)) else { continue };
        let args: Vec<&str> = cmd.iter().map(String::as_str).collect();
        if let Ok((false, o)) = ws.cmd(dir, &args) {
            return fail(format!("syntax check failed for {path}:\n{o}"));
        }
    }
    match ws.cmd(dir, test_cmd) {
        Ok((ok, o)) => Outcome { tests_pass: ok, lint_ok: true, output: o },
        Err(e) => Outcome { tests_pass: false, lint_ok: true, output: format!("test run failed: {e}") },
    }
}
