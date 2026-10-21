//! BUG_FIX patch-step quality (WP-B1, research memo C upgrades 1–3).
//!
//! * [`edits`]: anchored multi-file SEARCH/REPLACE edits, validated in memory
//!   before anything is written (replaces the single whole-file `{path, content}`).
//! * [`funnel`]: retrieval funnel (repo map → ranked candidate files → focused
//!   snippets) replacing "inline every tracked file up to 48 KB".
//! * [`select`]: N candidates evaluated in isolated checkouts, winner picked by
//!   tests → lint → normalized vote → smallest diff.
//!
//!
//! WP-B2 (memo C upgrades 4–8 + the rest of 3):
//! * [`repro`]: model-generated reproduction tests, kept only when they fail
//!   on the unpatched checkout; selection evidence, never a gate.
//! * [`baseline`]: regression gating on the baseline failure set, with
//!   bounded reruns; flakes recorded in the receipt.
//! * [`judge`]: a model judge that only orders candidates still tied after tests.
//! * [`explore`]: bounded read-only exploration on low localization
//!   confidence, and the optional planner step.
//! * `exec_hooks.rs`: the executor-side glue (a child module of the executor,
//!   so every model call and effect keeps its gate, caps, budget and journal).
//!   Prepare steps run concurrently (localization ∥ reproduction), prompts keep
//!   a stable prefix first.
//!
//! The executor (`super::executor`) keeps the gate, receipts, budget and caps;
//! this module only produces and ranks patches.

pub mod baseline;
pub mod edits;
pub mod explore;
pub mod funnel;
pub mod judge;
pub mod repro;
pub mod select;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_b2;

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

/// WP-B2: what candidate evaluation needs beyond the patch: kept
/// reproduction scripts (path, content), their runner argv prefix, and the
/// baseline suite output.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    pub repros: Vec<(String, String)>,
    pub runner: Vec<String>,
    pub baseline: String,
}

/// Evaluate candidates in isolated checkouts, in parallel: a local clone of
/// the run's checkout per candidate (cleaned up after), the patch written,
/// a syntax check per changed file, then the test command, then (WP-B2) the
/// kept reproduction scripts and a baseline comparison. Runs inside one gated
/// effect of the executor.
pub(crate) fn evaluate(ws: &Ws, attempt: u32, planned: &[(usize, &Planned)], test_cmd: &[&str], x: &Extras) -> Vec<Outcome> {
    let base = ws.root.join("candidates");
    let _ = std::fs::create_dir_all(&base);
    let out = std::thread::scope(|s| {
        let hs: Vec<_> = planned.iter().map(|(i, p)| {
            let dir = base.join(format!("a{attempt}-c{i}"));
            s.spawn(move || {
                let r = evaluate_one(ws, &dir, p, test_cmd, x);
                let _ = std::fs::remove_dir_all(&dir);
                r
            })
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap_or(Outcome::failed("candidate evaluation panicked", false))).collect()
    });
    let _ = std::fs::remove_dir_all(&base);
    out
}

fn evaluate_one(ws: &Ws, dir: &Path, p: &Planned, test_cmd: &[&str], x: &Extras) -> Outcome {
    let fail = |m: String| Outcome::failed(m, false);
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
    let (ok, output) = match ws.cmd(dir, test_cmd) {
        Ok(r) => r,
        Err(e) => return Outcome::failed(format!("test run failed: {e}"), true),
    };
    let regressions = if ok { 0 } else { baseline::new_failures(&baseline::parse(&x.baseline), &baseline::parse(&output)).len() };
    Outcome { tests_pass: ok, lint_ok: true, output, repro_pass: run_repros(ws, dir, x), regressions }
}

/// Write the kept reproduction scripts into `dir` (fresh paths only, never
/// over an existing file) and run each; returns (passed count). Used for
/// candidates and for the final checkout.
pub(crate) fn run_repros(ws: &Ws, dir: &Path, x: &Extras) -> usize {
    let mut passed = 0;
    for (path, content) in &x.repros {
        if !path.starts_with(repro::DIR) || x.runner.is_empty() {
            continue;
        }
        let f = dir.join(path);
        if !f.exists() {
            let _ = f.parent().map(std::fs::create_dir_all);
            if std::fs::write(&f, content).is_err() { continue; }
        } else if std::fs::read_to_string(&f).ok().as_deref() != Some(content.as_str()) {
            continue; // an existing different file: never overwrite
        }
        let mut args: Vec<&str> = x.runner.iter().map(String::as_str).collect();
        args.push(path);
        if let Ok((true, _)) = ws.cmd(dir, &args) {
            passed += 1;
        }
    }
    passed
}
