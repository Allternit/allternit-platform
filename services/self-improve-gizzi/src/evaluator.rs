//! The evaluator: the candidate's Gizzi runs the protected suite, and this
//! turns the report into per-case scores for `StagedGate`.
//!
//! Scores, per case, against the baseline report:
//! - pass: +1
//! - fail that also failed in the baseline: +0.5
//! - fail that passed in the baseline (a regression): -1
//!
//! `StagedGate` needs every score above 0, so "no case regressed" is the
//! gate. Staged results are cached by case id, and the full pass skips them.
//!
//! Price first: before any run the evaluator asks `eval estimate` and prints
//! the price. It asks for a yes unless `--yes` was given, and it stops with
//! an error when `--max-usd` would be passed.

use allternit_self_improve::eval::{EvalError, EvalReport, Evaluator};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Allowed drop in the suite pass rate, in percentage points
/// (same default as `scripts/agency-eval-gate.mjs`).
pub const MAX_DROP_PP: f64 = 2.0;
/// Flag that tells the candidate's `eval run` to leave out cases already run.
pub const SKIP_CASE_FLAG: &str = "--skip-case";

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)] // cost, tokens and ms are read from the report for the record
pub struct CaseResult {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub pass: bool,
    #[serde(default)]
    pub passes: u32,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub ms: u64,
    /// The next fields are read from the baseline to write the proposer brief.
    #[serde(default)]
    pub split: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub grader_details: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SuiteStat {
    pub pass_rate: Option<f64>,
    #[serde(default)]
    pub failed_cases: Vec<String>,
    #[serde(default)]
    pub high_stakes_failures: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Extra {
    #[serde(default)]
    pub per_case: Vec<CaseResult>,
}

/// An `allternit.eval_report.v1` report (the fields this tool reads).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Report {
    #[serde(default)]
    pub suites: BTreeMap<String, SuiteStat>,
    #[serde(default)]
    pub extra: Extra,
}

pub fn load_report(path: &Path) -> anyhow::Result<Report> {
    let text = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("could not read the baseline report {}: {e}", path.display()))?;
    parse_report(&text).map_err(|e| anyhow::anyhow!("{} is not an eval report: {e}", path.display()))
}

/// Read the first JSON object in `text` (the CLI may print log lines first).
pub fn parse_report(text: &str) -> Result<Report, String> {
    let start = text.find('{').ok_or("no JSON in the output")?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Report>();
    match stream.next() {
        Some(Ok(report)) => Ok(report),
        Some(Err(e)) => Err(e.to_string()),
        None => Err("empty output".into()),
    }
}

/// Pure port of `compareReports` from `scripts/agency-eval-gate.mjs`:
/// a suite may not drop more than `max_drop_pp` below its baseline, and no
/// suite may have a high-stakes failure. Returns the failure messages.
pub fn compare_reports(report: &BTreeMap<String, SuiteStat>, baseline: &BTreeMap<String, SuiteStat>, max_drop_pp: f64) -> Vec<String> {
    let mut failures = Vec::new();
    for (name, base) in baseline {
        let Some(cur) = report.get(name) else {
            failures.push(format!("{name}: missing from the report"));
            continue;
        };
        let (Some(cur_rate), Some(base_rate)) = (cur.pass_rate, base.pass_rate) else {
            failures.push(format!("{name}: nothing graded"));
            continue;
        };
        let drop = (base_rate - cur_rate) * 100.0;
        if drop > max_drop_pp + 1e-9 {
            failures.push(format!(
                "{name}: pass rate {:.1}% is {drop:.1} pp below the baseline {:.1}% (allowed {max_drop_pp} pp)",
                cur_rate * 100.0,
                base_rate * 100.0
            ));
        }
    }
    for (name, cur) in report {
        if !cur.high_stakes_failures.is_empty() {
            failures.push(format!("{name}: high-stakes case(s) failed: {}", cur.high_stakes_failures.join(", ")));
        }
    }
    failures
}

/// One case in the final table.
#[derive(Debug, Clone)]
pub struct CaseLine {
    pub id: String,
    pub label: String,
    pub pass: bool,
    pub base_pass: Option<bool>,
    pub score: f64,
}

/// What the evaluator saw, for the accept rule and `report.md`.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub spent_usd: f64,
    pub base_passes: usize,
    pub new_passes: usize,
    pub cases: Vec<CaseLine>,
    pub compare_failures: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub accepted: bool,
    pub reasons: Vec<String>,
}

/// The accept rule, applied after `StagedGate::run`: the gate is valid, the
/// compare is ok, and at least one more case passes than in the baseline.
pub fn decide(gate_valid: bool, summary: &Summary) -> Decision {
    let mut reasons = Vec::new();
    if !gate_valid {
        reasons.push("the gate did not pass (invalid patch, or a case regressed)".to_string());
    }
    for failure in &summary.compare_failures {
        reasons.push(format!("compare: {failure}"));
    }
    if summary.new_passes < summary.base_passes + 1 {
        reasons.push(format!("needs at least 1 more pass than the baseline ({} now, {} before)", summary.new_passes, summary.base_passes));
    }
    Decision { accepted: reasons.is_empty(), reasons }
}

/// An evaluator that can also say what it saw.
pub trait ScoredEvaluator: Send + Sync {
    fn eval_subset(&self, agent: &Path, fraction: f64) -> allternit_self_improve::eval::Result<EvalReport>;
    fn summary(&self) -> Summary;
    /// A new candidate is next: forget the cached cases, keep the spend.
    fn next_candidate(&self);
}

/// Lets `StagedGate` (which wants `&dyn Evaluator`) use a `ScoredEvaluator`.
pub struct AsEvaluator<'a>(pub &'a dyn ScoredEvaluator);

impl Evaluator for AsEvaluator<'_> {
    fn eval_subset(&self, agent: &Path, fraction: f64) -> allternit_self_improve::eval::Result<EvalReport> {
        self.0.eval_subset(agent, fraction)
    }
}

/// How the candidate's Gizzi CLI is started.
#[derive(Debug, Clone)]
pub enum Launcher {
    /// `bun <candidate>/cmd/gizzi-code/script/dev.mjs ...`
    Dev,
    /// A program that takes the `eval ...` arguments directly (tests).
    Program(PathBuf),
}

#[derive(Debug, Clone)]
pub struct EvalSettings {
    pub launcher: Launcher,
    /// The pristine suite folder. Never the candidate's copy.
    pub suite_dir: PathBuf,
    pub baseline: Report,
    pub seed: u64,
    pub model: Option<String>,
    pub yes: bool,
    pub max_usd: Option<f64>,
}

#[derive(Default)]
struct State {
    /// Dev cases from the staged pass, by id.
    cache: HashMap<String, CaseResult>,
    /// Results of the full pass (cached + new), once it ran.
    full: Option<Vec<CaseResult>>,
    full_suites: BTreeMap<String, SuiteStat>,
    spent: f64,
}

pub struct GizziEvaluator {
    settings: EvalSettings,
    state: Mutex<State>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Estimate {
    #[serde(default)]
    attempts: u64,
    #[serde(default)]
    minutes_est: Option<f64>,
    #[serde(default)]
    usd_est: Option<f64>,
    #[serde(default)]
    billing: Option<String>,
    #[serde(default)]
    probe_required: bool,
}

fn fail(msg: impl Into<String>) -> EvalError {
    EvalError::Evaluator(msg.into())
}

impl GizziEvaluator {
    pub fn new(settings: EvalSettings) -> Self {
        Self { settings, state: Mutex::new(State::default()) }
    }

    fn base_pass(&self, id: &str) -> Option<bool> {
        self.settings.baseline.extra.per_case.iter().find(|c| c.id == id).map(|c| c.pass)
    }

    fn score(&self, case: &CaseResult) -> f64 {
        score_case(case.pass, self.base_pass(&case.id))
    }

    fn command(&self, candidate: &Path, sub: &str, split: &str, fraction: f64, skip: &[String], json_yes: bool) -> Command {
        let mut cmd = match &self.settings.launcher {
            Launcher::Dev => {
                let mut c = Command::new("bun");
                c.arg(candidate.join("cmd/gizzi-code/script/dev.mjs"));
                c
            }
            Launcher::Program(program) => Command::new(program),
        };
        cmd.args(["eval", sub, "--mode", "live", "--suite-dir"])
            .arg(&self.settings.suite_dir)
            .args(["--split", split, "--fraction", &fraction.to_string(), "--seed", &self.settings.seed.to_string(), "--json"]);
        if json_yes {
            cmd.arg("--yes");
        }
        if let Some(model) = &self.settings.model {
            cmd.args(["--model", model]);
        }
        for id in skip {
            cmd.args([SKIP_CASE_FLAG, id]);
        }
        cmd
    }

    /// Ask for the price, print it, and stop if it is not allowed.
    fn price_first(&self, candidate: &Path, split: &str, fraction: f64, skip: &[String]) -> Result<(), EvalError> {
        let out = self
            .command(candidate, "estimate", split, fraction, skip, false)
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| fail(format!("could not start the eval estimate: {e}")))?;
        if !out.status.success() {
            return Err(fail("the eval estimate failed; no run was started"));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let start = text.find('{').ok_or_else(|| fail("the eval estimate printed no JSON"))?;
        let est: Estimate = serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Estimate>()
            .next()
            .ok_or_else(|| fail("the eval estimate printed nothing"))?
            .map_err(|e| fail(format!("could not read the eval estimate: {e}")))?;
        let price = match est.usd_est {
            Some(usd) => format!("about ${usd:.2}"),
            None => format!("no dollar price (billing: {})", est.billing.as_deref().unwrap_or("unknown")),
        };
        eprintln!(
            "eval price ({split}, fraction {fraction}): {} attempts, {}, {} min{}",
            est.attempts,
            price,
            est.minutes_est.map(|m| format!("{m:.0}")).unwrap_or_else(|| "?".into()),
            if est.probe_required { ", a probe run comes first" } else { "" }
        );
        let spent = self.state.lock().unwrap().spent;
        if let (Some(max), Some(usd)) = (self.settings.max_usd, est.usd_est) {
            if spent + usd > max {
                return Err(fail(format!("--max-usd ${max:.2} would be passed: ${spent:.2} spent, this run is about ${usd:.2}. Nothing was run.")));
            }
        }
        if !self.settings.yes {
            eprint!("run it? [y/N] ");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            if !matches!(line.trim(), "y" | "Y" | "yes") {
                return Err(fail("not confirmed; nothing was run (pass --yes to skip this question)"));
            }
        }
        Ok(())
    }

    fn run(&self, candidate: &Path, split: &str, fraction: f64, skip: &[String]) -> Result<Report, EvalError> {
        self.price_first(candidate, split, fraction, skip)?;
        let out = self
            .command(candidate, "run", split, fraction, skip, true)
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| fail(format!("could not start the eval run: {e}")))?;
        if !out.status.success() {
            return Err(fail(format!("the eval run failed ({})", out.status)));
        }
        let report = parse_report(&String::from_utf8_lossy(&out.stdout)).map_err(|e| fail(format!("could not read the eval report: {e}")))?;
        let cost: f64 = report.extra.per_case.iter().map(|c| c.cost).sum();
        let mut state = self.state.lock().unwrap();
        state.spent += cost;
        if let Some(max) = self.settings.max_usd {
            if state.spent > max {
                return Err(fail(format!("--max-usd ${max:.2} was passed: ${:.2} spent. Stopped.", state.spent)));
            }
        }
        Ok(report)
    }
}

pub fn score_case(pass: bool, base_pass: Option<bool>) -> f64 {
    match (pass, base_pass) {
        (true, _) => 1.0,
        (false, Some(true)) => -1.0,
        (false, _) => 0.5,
    }
}

impl ScoredEvaluator for GizziEvaluator {
    fn eval_subset(&self, agent: &Path, fraction: f64) -> allternit_self_improve::eval::Result<EvalReport> {
        if fraction < 1.0 {
            let report = self.run(agent, "dev", fraction, &[])?;
            let mut state = self.state.lock().unwrap();
            let mut scores = HashMap::new();
            for case in report.extra.per_case {
                scores.insert(case.id.clone(), self.score(&case));
                state.cache.insert(case.id.clone(), case);
            }
            return Ok(EvalReport { scores, valid: true, fraction });
        }
        // Full gate: dev and holdout. Staged cases are not run again.
        let skip: Vec<String> = {
            let state = self.state.lock().unwrap();
            if let Some(done) = &state.full {
                let scores = done.iter().map(|c| (c.id.clone(), self.score(c))).collect();
                return Ok(EvalReport { scores, valid: true, fraction: 1.0 });
            }
            let mut ids: Vec<String> = state.cache.keys().cloned().collect();
            ids.sort();
            ids
        };
        let report = self.run(agent, "all", 1.0, &skip)?;
        let mut state = self.state.lock().unwrap();
        let mut merged: Vec<CaseResult> = state.cache.values().cloned().collect();
        merged.extend(report.extra.per_case);
        merged.sort_by(|a, b| a.id.cmp(&b.id));
        let scores = merged.iter().map(|c| (c.id.clone(), self.score(c))).collect();
        state.full_suites = report.suites;
        state.full = Some(merged);
        Ok(EvalReport { scores, valid: true, fraction: 1.0 })
    }

    fn next_candidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.cache.clear();
        state.full = None;
        state.full_suites.clear();
    }

    fn summary(&self) -> Summary {
        let state = self.state.lock().unwrap();
        let spent_usd = state.spent;
        let mut results: Vec<CaseResult> = match &state.full {
            Some(full) => full.clone(),
            None => state.cache.values().cloned().collect(),
        };
        results.sort_by(|a, b| a.id.cmp(&b.id));
        let cases: Vec<CaseLine> = results
            .iter()
            .map(|c| CaseLine { id: c.id.clone(), label: c.label.clone(), pass: c.pass, base_pass: self.base_pass(&c.id), score: self.score(c) })
            .collect();
        // No full pass means the gate stopped early: no accept.
        let (mut new_passes, mut base_passes, mut compare_failures) = (0, 0, Vec::new());
        if state.full.is_some() {
            new_passes = cases.iter().filter(|c| c.pass).count();
            base_passes = cases.iter().filter(|c| c.base_pass == Some(true)).count();
            let total = cases.len().max(1) as f64;
            let current = SuiteStat {
                pass_rate: Some(new_passes as f64 / total),
                failed_cases: cases.iter().filter(|c| !c.pass).map(|c| c.id.clone()).collect(),
                high_stakes_failures: state.full_suites.values().flat_map(|s| s.high_stakes_failures.clone()).collect(),
            };
            let base = SuiteStat { pass_rate: Some(base_passes as f64 / total), ..Default::default() };
            compare_failures = compare_reports(
                &BTreeMap::from([("all".to_string(), current)]),
                &BTreeMap::from([("all".to_string(), base)]),
                MAX_DROP_PP,
            );
        }
        Summary { spent_usd, base_passes, new_passes, cases, compare_failures }
    }
}

/// Canned evaluator for `--dry-run`: no process, no network, no price.
pub struct FakeEvaluator;

impl ScoredEvaluator for FakeEvaluator {
    fn eval_subset(&self, _agent: &Path, fraction: f64) -> allternit_self_improve::eval::Result<EvalReport> {
        let scores = HashMap::from([("fake-1".to_string(), 1.0), ("fake-2".to_string(), 0.5)]);
        Ok(EvalReport { scores, valid: true, fraction })
    }

    fn next_candidate(&self) {}

    fn summary(&self) -> Summary {
        Summary {
            spent_usd: 0.0,
            base_passes: 0,
            new_passes: 1,
            cases: vec![
                CaseLine { id: "fake-1".into(), label: "dry run".into(), pass: true, base_pass: Some(false), score: 1.0 },
                CaseLine { id: "fake-2".into(), label: "dry run".into(), pass: false, base_pass: Some(false), score: 0.5 },
            ],
            compare_failures: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_self_improve::eval::{StagedGate, ValidityCheck};
    use std::os::unix::fs::PermissionsExt;

    struct Valid;
    impl ValidityCheck for Valid {
        fn check(&self, _agent: &Path) -> allternit_self_improve::eval::Result<bool> {
            Ok(true)
        }
    }

    fn report_json(cases: &[(&str, bool, f64)]) -> String {
        let per_case: Vec<String> = cases
            .iter()
            .map(|(id, pass, cost)| format!(r#"{{"id":"{id}","label":"l","pass":{pass},"passes":1,"attempts":1,"cost":{cost},"tokens":1,"ms":1}}"#))
            .collect();
        let passed = cases.iter().filter(|c| c.1).count() as f64 / cases.len().max(1) as f64;
        format!(r#"{{"schema":"allternit.eval_report.v1","suites":{{"s":{{"pass_rate":{passed},"failed_cases":[],"high_stakes_failures":[]}}}},"extra":{{"per_case":[{}]}}}}"#, per_case.join(","))
    }

    /// A fake gizzi: prints canned JSON by subcommand and split, and logs
    /// every call's arguments to `calls.log`.
    struct Fake {
        dir: tempfile::TempDir,
    }
    impl Fake {
        fn new(dev: &str, all: &str, usd_est: f64) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let d = dir.path();
            std::fs::write(d.join("dev.json"), dev).unwrap();
            std::fs::write(d.join("all.json"), all).unwrap();
            std::fs::write(d.join("estimate.json"), format!(r#"{{"attempts":3,"tokens_est":10,"minutes_est":1,"billing":"api","usd_est":{usd_est},"probe_required":false}}"#)).unwrap();
            let script = format!(
                "#!/bin/sh\necho \"$@\" >> '{0}/calls.log'\nsplit=''\nwhile [ $# -gt 0 ]; do if [ \"$1\" = --split ]; then split=$2; fi; shift; done\ncase \"$(tail -n1 '{0}/calls.log')\" in\n  eval\\ estimate*) cat '{0}/estimate.json';;\n  *) cat \"{0}/$split.json\";;\nesac\n",
                d.display()
            );
            let path = d.join("gizzi");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }
        fn evaluator(&self, baseline: &str, max_usd: Option<f64>) -> GizziEvaluator {
            GizziEvaluator::new(EvalSettings {
                launcher: Launcher::Program(self.dir.path().join("gizzi")),
                suite_dir: PathBuf::from("/pristine/suite"),
                baseline: parse_report(baseline).unwrap(),
                seed: 7,
                model: None,
                yes: true,
                max_usd,
            })
        }
        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default().lines().map(str::to_owned).collect()
        }
        fn runs(&self, split: &str) -> usize {
            self.calls().iter().filter(|l| l.starts_with("eval run") && l.contains(&format!("--split {split} "))).count()
        }
    }

    #[test]
    fn a_regression_blocks_the_full_gate() {
        let baseline = report_json(&[("a", true, 0.0), ("b", false, 0.0)]);
        let fake = Fake::new(&report_json(&[("a", false, 0.1), ("b", true, 0.1)]), &report_json(&[("c", true, 0.1)]), 0.5);
        let ev = fake.evaluator(&baseline, None);
        let report = StagedGate::new(&Valid, &AsEvaluator(&ev), 0.5).run(Path::new("/cand")).unwrap();
        assert!(!report.valid);
        assert_eq!(report.scores["a"], -1.0, "passed before, fails now");
        assert_eq!(report.scores["b"], 1.0);
        assert_eq!(fake.runs("all"), 0, "the full gate must not start after a regression");
        assert!(!decide(report.valid, &ev.summary()).accepted);
    }

    #[test]
    fn staged_cases_are_not_run_again_in_the_full_pass() {
        let baseline = report_json(&[("a", true, 0.0), ("b", false, 0.0), ("c", false, 0.0)]);
        let fake = Fake::new(&report_json(&[("a", true, 0.1), ("b", true, 0.1)]), &report_json(&[("c", false, 0.1)]), 0.5);
        let ev = fake.evaluator(&baseline, None);
        let report = StagedGate::new(&Valid, &AsEvaluator(&ev), 0.5).run(Path::new("/cand")).unwrap();
        assert!(report.valid);
        assert_eq!(fake.runs("dev"), 1);
        assert_eq!(fake.runs("all"), 1);
        let full_call = fake.calls().into_iter().find(|l| l.starts_with("eval run") && l.contains("--split all ")).unwrap();
        assert!(full_call.contains("--skip-case a") && full_call.contains("--skip-case b"), "{full_call}");
        assert!(!full_call.contains("--skip-case c"));
        assert_eq!(report.scores.len(), 3, "the full report merges cached and new cases");
        // a repeat full call reuses everything
        ev.eval_subset(Path::new("/cand"), 1.0).unwrap();
        assert_eq!(fake.runs("all"), 1);
    }

    #[test]
    fn max_usd_stops_the_run_before_it_starts() {
        let baseline = report_json(&[("a", false, 0.0)]);
        let fake = Fake::new(&report_json(&[("a", true, 0.0)]), &report_json(&[]), 5.0);
        let ev = fake.evaluator(&baseline, Some(1.0));
        let err = StagedGate::new(&Valid, &AsEvaluator(&ev), 0.5).run(Path::new("/cand")).unwrap_err().to_string();
        assert!(err.contains("--max-usd"), "{err}");
        assert_eq!(fake.runs("dev"), 0, "no run may start when the price is over the cap");
    }

    #[test]
    fn max_usd_stops_when_the_reports_show_the_spend() {
        let baseline = report_json(&[("a", false, 0.0)]);
        let fake = Fake::new(&report_json(&[("a", true, 3.0)]), &report_json(&[]), 0.1);
        let ev = fake.evaluator(&baseline, Some(1.0));
        let err = StagedGate::new(&Valid, &AsEvaluator(&ev), 0.5).run(Path::new("/cand")).unwrap_err().to_string();
        assert!(err.contains("--max-usd"), "{err}");
    }

    #[test]
    fn accept_needs_one_more_pass_than_the_baseline() {
        let baseline = report_json(&[("a", true, 0.0), ("b", false, 0.0)]);
        // nothing regressed, nothing gained
        let same = Fake::new(&report_json(&[("a", true, 0.1)]), &report_json(&[("b", false, 0.1)]), 0.1);
        let ev = same.evaluator(&baseline, None);
        let report = StagedGate::new(&Valid, &AsEvaluator(&ev), 0.5).run(Path::new("/c")).unwrap();
        let decision = decide(report.valid, &ev.summary());
        assert!(report.valid && !decision.accepted, "{decision:?}");
        assert!(decision.reasons.iter().any(|r| r.contains("1 more pass")));
        // one gained
        let better = Fake::new(&report_json(&[("a", true, 0.1)]), &report_json(&[("b", true, 0.1)]), 0.1);
        let ev = better.evaluator(&baseline, None);
        let report = StagedGate::new(&Valid, &AsEvaluator(&ev), 0.5).run(Path::new("/c")).unwrap();
        assert!(decide(report.valid, &ev.summary()).accepted);
    }

    #[test]
    fn compare_blocks_a_high_stakes_failure_and_a_big_drop() {
        let stat = |rate, high: &[&str]| SuiteStat { pass_rate: Some(rate), failed_cases: vec![], high_stakes_failures: high.iter().map(|s| s.to_string()).collect() };
        let base = BTreeMap::from([("s".to_string(), stat(0.9, &[]))]);
        let ok = BTreeMap::from([("s".to_string(), stat(0.89, &[]))]);
        assert!(compare_reports(&ok, &base, MAX_DROP_PP).is_empty(), "1 pp is allowed");
        let drop = BTreeMap::from([("s".to_string(), stat(0.80, &[]))]);
        assert!(compare_reports(&drop, &base, MAX_DROP_PP)[0].contains("below the baseline"));
        let high = BTreeMap::from([("s".to_string(), stat(0.95, &["hs-1"]))]);
        assert!(compare_reports(&high, &base, MAX_DROP_PP)[0].contains("high-stakes"));
    }
}
