//! N parallel candidates + selection (memo C, upgrade 3).
//!
//! Each candidate patch is applied to its own disposable checkout, syntax-
//! checked and tested there; the winner is picked by test result, then the
//! lint/parse gate, then a whitespace-normalized majority vote, then the
//! smallest diff, then generation order (deterministic).

use super::edits::Planned;
use std::path::Path;

/// One candidate's evaluation in its isolated checkout.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub tests_pass: bool,
    pub lint_ok: bool,
    pub output: String,
}

/// Index of the winning candidate. `outcomes[i]` belongs to `planned[i]`.
pub fn pick(planned: &[Planned], outcomes: &[Outcome]) -> Option<usize> {
    let keys: Vec<String> = planned.iter().map(Planned::normalized_key).collect();
    let votes = |i: usize| keys.iter().filter(|k| **k == keys[i]).count();
    (0..planned.len().min(outcomes.len())).max_by(|&a, &b| {
        let (oa, ob) = (&outcomes[a], &outcomes[b]);
        oa.tests_pass.cmp(&ob.tests_pass)
            .then(oa.lint_ok.cmp(&ob.lint_ok))
            .then(votes(a).cmp(&votes(b)))
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
        format!("{i}={}", if o.tests_pass { "PASS" } else if o.lint_ok { "FAIL" } else { "LINT" })
    }).collect();
    format!("candidates:{attempt}:{}", parts.join(","))
}

pub fn decode(evidence: &str) -> Vec<(usize, Outcome)> {
    let list = evidence.splitn(3, ':').nth(2).unwrap_or_default();
    let list = list.split(':').next().unwrap_or_default();
    list.split(',').filter_map(|p| {
        let (i, r) = p.split_once('=')?;
        Some((i.parse().ok()?, Outcome { tests_pass: r == "PASS", lint_ok: r != "LINT", output: String::new() }))
    }).collect()
}
