//! Regression baseline and flakiness (memo C, upgrade 6).
//!
//! The "before" run of the suite (N10) is the baseline: a later run is gated
//! only on failures that are not in the baseline failure set. Failing tests
//! that differ from the baseline are rerun (whole suite, bounded) and a test
//! that passes on any rerun is recorded as a flake in the receipt, not counted
//! as a regression. Per-test results are parsed from common runner output;
//! when nothing parses, the strict whole-suite result stands (fail closed).

use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Suite reruns for tests that differ from the baseline.
pub const RERUNS: usize = 2;

fn re() -> &'static [Regex; 4] {
    static R: OnceLock<[Regex; 4]> = OnceLock::new();
    R.get_or_init(|| [
        // TAP (node --test, tap): "ok 1 - name" / "not ok 2 - name"
        Regex::new(r"(?m)^\s*(not )?ok \d+ - (.+?)\s*(?:#.*)?$").unwrap(),
        // cargo test: "test path::name ... ok|FAILED"
        Regex::new(r"(?m)^test (\S+) \.\.\. (ok|FAILED)\s*$").unwrap(),
        // pytest summary: "FAILED tests/x.py::name - reason" / "PASSED ..."
        Regex::new(r"(?m)^(FAILED|ERROR|PASSED) (\S+)").unwrap(),
        // jest / vitest: "✓ name" / "✕ name (3 ms)"
        Regex::new(r"(?m)^\s*([✓✔√]|[✕×✗]) (.+?)(?: \(\d+ ?m?s\))?\s*$").unwrap(),
    ])
}

/// Per-test results (name → passed) found in a runner's output.
pub fn parse(out: &str) -> BTreeMap<String, bool> {
    let [tap, cargo, pytest, jest] = re();
    let mut m = BTreeMap::new();
    for c in tap.captures_iter(out) {
        m.insert(c[2].to_string(), c.get(1).is_none());
    }
    for c in cargo.captures_iter(out) {
        m.insert(c[1].to_string(), &c[2] == "ok");
    }
    for c in pytest.captures_iter(out) {
        m.insert(c[2].to_string(), &c[1] == "PASSED");
    }
    for c in jest.captures_iter(out) {
        m.insert(c[2].to_string(), matches!(&c[1], "✓" | "✔" | "√"));
    }
    m
}

/// Failures in `after` that the baseline did not already have.
pub fn new_failures(before: &BTreeMap<String, bool>, after: &BTreeMap<String, bool>) -> Vec<String> {
    after.iter().filter(|(t, ok)| !**ok && before.get(*t) != Some(&false)).map(|(t, _)| t.clone()).collect()
}

/// Gate verdict for a non-green suite run, given the baseline output and the
/// reruns' outputs: (passes the baseline gate, flaky tests, real regressions).
/// Nothing parsed = fail closed.
pub fn judge_run(baseline: &str, after: &str, reruns: &[String]) -> (bool, Vec<String>, Vec<String>) {
    let (b, a) = (parse(baseline), parse(after));
    if a.is_empty() || !a.values().any(|ok| !ok) {
        return (false, vec![], vec![]);
    }
    let (mut flaky, mut real) = (vec![], vec![]);
    for t in new_failures(&b, &a) {
        let passed_later = reruns.iter().any(|r| {
            let rr = parse(r);
            rr.get(&t) == Some(&true) || (!rr.is_empty() && !rr.contains_key(&t) && !rr.values().any(|ok| !ok))
        });
        if passed_later { flaky.push(t) } else { real.push(t) }
    }
    (real.is_empty(), flaky, real)
}
