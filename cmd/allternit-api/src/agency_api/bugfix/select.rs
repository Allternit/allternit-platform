//! N parallel candidates + selection (memo C, upgrade 3; WP-B2 adds 5/6 + judge).
//!
//! Each candidate patch is applied to its own disposable checkout, syntax-
//! checked and tested there; the winner is picked by test result, then the
//! lint/parse gate, then model-generated reproduction tests passed (evidence,
//! never a gate), then fewest new failures vs the baseline, then a
//! whitespace-normalized majority vote, then the judge's pick among the
//! candidates still tied (tests decide first), then the smallest diff, then
//! generation order (deterministic).

use super::edits::Planned;
use std::path::Path;

/// One candidate's evaluation in its isolated checkout.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub tests_pass: bool,
    pub lint_ok: bool,
    pub output: String,
    /// WP-B2: kept reproduction tests this candidate makes pass.
    pub repro_pass: usize,
    /// WP-B2: failures not in the baseline failure set (new regressions).
    pub regressions: usize,
}

impl Outcome {
    pub fn failed(output: impl Into<String>, lint_ok: bool) -> Self {
        Outcome { tests_pass: false, lint_ok, output: output.into(), repro_pass: 0, regressions: 0 }
    }
}

/// Normalized-outcome vote count per candidate within `planned`.
pub fn votes_of(planned: &[Planned]) -> Vec<usize> {
    let keys: Vec<String> = planned.iter().map(Planned::normalized_key).collect();
    keys.iter().map(|k| keys.iter().filter(|x| *x == k).count()).collect()
}

/// The cascade up to (not including) the judge: tests, lint, repro, regressions, votes.
fn cascade(o: &Outcome, votes: usize) -> (bool, bool, usize, std::cmp::Reverse<usize>, usize) {
    (o.tests_pass, o.lint_ok, o.repro_pass, std::cmp::Reverse(o.regressions), votes)
}

/// Candidates still tied at the top of the cascade after the tests decided
/// (only when the best passes its tests; the judge never rescues a failure).
pub fn tied(outcomes: &[Outcome], votes: &[usize]) -> Vec<usize> {
    let n = outcomes.len().min(votes.len());
    let Some(best) = (0..n).map(|i| cascade(&outcomes[i], votes[i])).max() else { return vec![] };
    if !best.0 {
        return vec![];
    }
    (0..n).filter(|&i| cascade(&outcomes[i], votes[i]) == best).collect()
}

/// Index of the winning candidate. `outcomes[i]` belongs to `planned[i]`.
pub fn pick(planned: &[Planned], outcomes: &[Outcome]) -> Option<usize> {
    pick_with(planned, outcomes, &votes_of(planned), None)
}

/// [`pick`] with explicit vote counts (from [`dedupe`]: the executor only
/// evaluates distinct candidates, so votes come from the full set) and the
/// judge's choice among [`tied`] candidates.
pub fn pick_with(planned: &[Planned], outcomes: &[Outcome], votes: &[usize], judge: Option<usize>) -> Option<usize> {
    let tie = tied(outcomes, votes);
    let judged = |i: usize| judge == Some(i) && tie.contains(&i);
    (0..planned.len().min(outcomes.len()).min(votes.len())).max_by(|&a, &b| {
        cascade(&outcomes[a], votes[a]).cmp(&cascade(&outcomes[b], votes[b]))
            .then(judged(a).cmp(&judged(b)))
            .then(planned[b].diff_lines.cmp(&planned[a].diff_lines))
            .then(b.cmp(&a))
    })
}

/// Drop candidates whose normalized outcome duplicates an earlier one (no
/// point testing the same change twice); returns kept indices and the vote
/// count each kept candidate carries.
pub fn dedupe(planned: &[Planned]) -> Vec<(usize, usize)> {
    let keys: Vec<String> = planned.iter().map(Planned::normalized_key).collect();
    (0..keys.len()).filter(|&i| !keys[..i].contains(&keys[i]))
        .map(|i| (i, keys.iter().filter(|k| **k == keys[i]).count())).collect()
}

/// Write a planned patch into a checkout (deletes included). Paths were
/// validated by [`super::edits::plan`].
pub fn write_planned(repo: &Path, p: &Planned) -> std::io::Result<()> {
    for (path, content) in &p.files {
        let f = repo.join(path);
        match content {
            Some(c) => {
                if let Some(d) = f.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&f, c)?;
            }
            None => {
                if f.exists() {
                    std::fs::remove_file(&f)?;
                }
            }
        }
    }
    Ok(())
}

/// Syntax-check command for a changed file, when a cheap one exists.
pub fn syntax_check(path: &str) -> Option<Vec<String>> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "js" | "cjs" | "mjs" => Some(vec!["node".into(), "--check".into(), path.into()]),
        "py" => Some(vec!["python3".into(), "-m".into(), "py_compile".into(), path.into()]),
        _ => None,
    }
}

/// In-memory parse gate for data files (JSON must still parse).
pub fn parse_ok(p: &Planned) -> bool {
    p.files.iter().all(|(path, c)| match (Path::new(path).extension().and_then(|e| e.to_str()), c) {
        (Some("json"), Some(c)) => serde_json::from_str::<serde_json::Value>(c).is_ok(),
        _ => true,
    })
}

/// Encode outcomes in the effect's evidence ref (so a re-driven run that hits
/// an already-committed effect recovers them), and decode them back.
pub fn encode(attempt: u32, kept: &[usize], outcomes: &[Outcome]) -> String {
    let parts: Vec<String> = kept.iter().zip(outcomes).map(|(i, o)| {
        format!("{i}={}/r{}/g{}", if o.tests_pass { "PASS" } else if o.lint_ok { "FAIL" } else { "LINT" }, o.repro_pass, o.regressions)
    }).collect();
    format!("candidates:{attempt}:{}", parts.join(","))
}

pub fn decode(evidence: &str) -> Vec<(usize, Outcome)> {
    let list = evidence.splitn(3, ':').nth(2).unwrap_or_default();
    let list = list.split(':').next().unwrap_or_default();
    list.split(',').filter_map(|p| {
        let (i, r) = p.split_once('=')?;
        let mut f = r.split('/');
        let r = f.next()?;
        let (mut repro_pass, mut regressions) = (0, 0);
        for x in f {
            if let Some(n) = x.strip_prefix('r') { repro_pass = n.parse().unwrap_or(0); }
            if let Some(n) = x.strip_prefix('g') { regressions = n.parse().unwrap_or(0); }
        }
        Some((i.parse().ok()?, Outcome { tests_pass: r == "PASS", lint_ok: r != "LINT", output: String::new(), repro_pass, regressions }))
    }).collect()
}
