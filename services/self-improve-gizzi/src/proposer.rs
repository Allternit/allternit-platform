//! The proposer, the report and the lineage record.
//!
//! The proposer runs the installed `gizzi run` on a working copy of the
//! harness view. It never sees the hidden tests, the check files or the
//! holdout cases: its brief is plain text built from the failed dev cases of
//! the baseline report.
//!
//! Gizzi has no flag that loads an agent prompt from a file (`--agent` only
//! names an agent it already knows), so the editor prompt
//! (`prompts/harness-editor.txt`) goes at the top of the message.
//!
//! `--open-pr` is the only step that touches a remote. It opens a DRAFT PR
//! with the allowlisted files only. This tool never merges a PR and never
//! runs `train ready`.

use crate::evaluator::{decide, AsEvaluator, Decision, Report, ScoredEvaluator, Summary};
use crate::validity::{self, BunGuardTests, GuardOutcome, GuardRunner, HarnessValidity};
use crate::view::{self, ViewDir};
use allternit_self_improve::eval::{EvalReport, StagedGate};
use allternit_self_improve::lineage::{Generation, LineageArchive};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The editor prompt, in our own words. Goes at the top of the message.
pub const EDITOR_PROMPT: &str = include_str!("../prompts/harness-editor.txt");
/// Lineage folder, under the repo.
pub const LINEAGE_DIR: &str = ".harness-improve/lineage";
/// Default output folder, under the repo.
pub const OUT_DIR: &str = ".harness-improve/out";

/// Failed dev cases from the baseline, as plain text. Only cases the baseline
/// marks as `dev` are listed; nothing from holdout and no check files.
pub fn build_brief(baseline: &Report) -> String {
    let failed: Vec<_> = baseline.extra.per_case.iter().filter(|c| !c.pass && c.split.as_deref() == Some("dev")).collect();
    if failed.is_empty() {
        return "No failing dev cases are known. Make no change unless you see a clear defect in the files.".to_string();
    }
    let mut out = format!("These {} dev cases fail today:\n", failed.len());
    for case in failed {
        out.push_str(&format!("\n- {}", case.title.as_deref().unwrap_or(&case.id)));
        if !case.label.is_empty() {
            out.push_str(&format!(" [{}]", case.label));
        }
        if let Some(note) = &case.note {
            out.push_str(&format!("\n  note: {note}"));
        }
        if let Some(details) = &case.grader_details {
            out.push_str(&format!("\n  what the grader saw: {details}"));
        }
    }
    out.push('\n');
    out
}

fn message(brief: &str) -> String {
    format!("{}\n---\n{brief}", EDITOR_PROMPT.trim_end())
}

/// Makes one repo-relative patch from the harness view.
pub trait Proposer {
    fn propose(&self, view: &ViewDir, brief: &str) -> Result<String>;
}

/// The installed `gizzi run` on a working copy of the view.
pub struct GizziProposer {
    pub model: Option<String>,
}

impl Proposer for GizziProposer {
    fn propose(&self, view: &ViewDir, brief: &str) -> Result<String> {
        let dest = tempfile::Builder::new().prefix("harness-edit-").tempdir()?;
        let copy = view.working_copy(dest.path())?;
        let bin = std::env::var("HARNESS_IMPROVE_GIZZI").unwrap_or_else(|_| "gizzi".to_string());
        let mut cmd = Command::new(&bin);
        cmd.args(["run", "--format", "json", "--dir"]).arg(copy.path());
        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }
        cmd.arg(message(brief));
        let out = cmd.output().with_context(|| format!("could not start `{bin} run`; is Gizzi installed?"))?;
        if !out.status.success() {
            bail!("`{bin} run` failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
        }
        let raw = copy.finalize_diff().context("could not read the proposed changes")?;
        Ok(view::normalize_view_diff(&raw, view.root(), copy.path()))
    }
}

/// Dry-run proposer: adds one plain line to the first prompt file.
pub struct FakeProposer;

impl Proposer for FakeProposer {
    fn propose(&self, view: &ViewDir, _brief: &str) -> Result<String> {
        let target = view
            .files()
            .iter()
            .find(|f| f.starts_with(view::PROMPT_DIR) && f.ends_with(".txt"))
            .context("the view has no prompt file to edit")?
            .clone();
        let dest = tempfile::Builder::new().prefix("harness-fake-").tempdir()?;
        let copy = view.working_copy(dest.path())?;
        let path = copy.path().join(&target);
        let mut text = std::fs::read_to_string(&path)?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("Check that a file exists before you edit it.\n");
        std::fs::write(&path, text)?;
        let raw = copy.finalize_diff()?;
        Ok(view::normalize_view_diff(&raw, view.root(), copy.path()))
    }
}

/// Guard runner for dry runs: the guard tests are not started.
pub struct SkipGuard;

impl GuardRunner for SkipGuard {
    fn run(&self, _checkout: &Path) -> Result<GuardOutcome> {
        Ok(GuardOutcome { passed: true, output: "skipped (dry run)".into() })
    }
}

/// Everything the gate learned about one patch.
#[derive(Debug)]
pub struct GateRun {
    pub report: EvalReport,
    pub summary: Summary,
    pub decision: Decision,
    pub rejections: Vec<String>,
}

/// Validity, staged eval, full eval, accept rule.
pub fn gate_patch(repo: &Path, patch: &str, evaluator: &dyn ScoredEvaluator, fake: bool, fraction: f64) -> Result<GateRun> {
    let invalid = |reasons: Vec<String>| GateRun {
        report: EvalReport::default(),
        summary: Summary::default(),
        decision: Decision { accepted: false, reasons: reasons.clone() },
        rejections: reasons,
    };
    if patch.trim().is_empty() {
        return Ok(invalid(vec!["the brain made no change".to_string()]));
    }
    if let Err(reasons) = validity::check_patch(patch, repo) {
        return Ok(invalid(reasons));
    }
    evaluator.next_candidate();
    // Keep the candidate alive until the gate is done.
    let mut _real = None;
    let mut _tmp = None;
    let candidate: PathBuf = if fake {
        let tmp = tempfile::Builder::new().prefix("harness-fake-candidate-").tempdir()?;
        view::export_view_into(repo, tmp.path())?;
        let path = tmp.path().to_path_buf();
        _tmp = Some(tmp);
        path
    } else {
        let checkout = view::CandidateCheckout::create(repo)?;
        let path = checkout.path().to_path_buf();
        _real = Some(checkout);
        path
    };
    view::apply_to_checkout(patch, &candidate)?;
    let guard: Box<dyn GuardRunner> = if fake { Box::new(SkipGuard) } else { Box::new(BunGuardTests) };
    let validity = HarnessValidity::new(repo, patch.to_string(), guard);
    let report = StagedGate::new(&validity, &AsEvaluator(evaluator), fraction).run(&candidate)?;
    let summary = evaluator.summary();
    let rejections = validity.rejections();
    let mut decision = decide(report.valid, &summary);
    decision.reasons.extend(rejections.iter().cloned());
    Ok(GateRun { report, summary, decision, rejections })
}

/// `report.md`: the gate scores per case, the price, the validity reasons
/// and the decision.
pub fn render_report(id: &str, patch: &str, run: &GateRun, brief: &str) -> String {
    let mut md = format!("# harness-improve {id}\n\n");
    md.push_str(&format!("Decision: **{}**\n\n", if run.decision.accepted { "accepted" } else { "not accepted" }));
    for reason in &run.decision.reasons {
        md.push_str(&format!("- {}\n", reason.replace('\n', " ")));
    }
    md.push_str(&format!("\nPrice paid: ${:.2}\n\n", run.summary.spent_usd));
    md.push_str(&format!(
        "Passes: {} now, {} in the baseline. Gate valid: {}.\n\n",
        run.summary.new_passes, run.summary.base_passes, run.report.valid
    ));
    md.push_str("## Gate scores per case\n\n| case | label | before | now | score |\n|---|---|---|---|---|\n");
    let word = |p: Option<bool>| match p {
        Some(true) => "pass",
        Some(false) => "fail",
        None => "n/a",
    };
    for case in &run.summary.cases {
        md.push_str(&format!("| {} | {} | {} | {} | {} |\n", case.id, case.label, word(case.base_pass), word(Some(case.pass)), case.score));
    }
    if run.summary.cases.is_empty() {
        md.push_str("| none ran |  |  |  |  |\n");
    }
    md.push_str("\n## Validity\n\n");
    if run.rejections.is_empty() {
        md.push_str("No validity problems.\n");
    }
    for reason in &run.rejections {
        md.push_str(&format!("- {}\n", reason.replace('\n', " ")));
    }
    md.push_str(&format!("\n## Brief given to the proposer\n\n{brief}\n## Patch\n\n```diff\n{patch}```\n"));
    md
}

pub struct Outcome {
    pub id: String,
    pub out_dir: PathBuf,
    pub accepted: bool,
    pub pr_url: Option<String>,
}

/// Write patch.diff and report.md, then record the generation.
pub fn record(repo: &Path, out_dir: &Path, id: &str, parent: Option<&str>, patch: &str, run: &GateRun, brief: &str) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join("patch.diff"), patch)?;
    std::fs::write(out_dir.join("report.md"), render_report(id, patch, run, brief))?;
    let mut archive = LineageArchive::open(repo.join(LINEAGE_DIR)).context("could not open the lineage archive")?;
    let mut generation = Generation::new(id, parent.map(str::to_owned));
    generation.valid = run.report.valid;
    generation.score = run.report.mean_score();
    archive.record(generation, &[("patch.diff", patch)]).context("could not record the lineage")?;
    Ok(())
}

pub struct LoopConfig<'a> {
    pub repo: &'a Path,
    pub out: Option<PathBuf>,
    pub fraction: f64,
    pub max_generations: u32,
    pub fake: bool,
    pub open_pr: bool,
    pub baseline: &'a Report,
}

/// Propose and gate, up to `max_generations` rounds. Stops at the first
/// accepted patch.
pub fn run_loop(cfg: &LoopConfig, proposer: &dyn Proposer, evaluator: &dyn ScoredEvaluator) -> Result<Vec<Outcome>> {
    let brief = build_brief(cfg.baseline);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut outcomes: Vec<Outcome> = Vec::new();
    for round in 1..=cfg.max_generations {
        let view = view::export_view(cfg.repo)?;
        let patch = proposer.propose(&view, &brief)?;
        let id = format!("g{stamp}-{round}");
        let run = gate_patch(cfg.repo, &patch, evaluator, cfg.fake, cfg.fraction)?;
        let base_out = cfg.out.clone().unwrap_or_else(|| cfg.repo.join(OUT_DIR));
        let out_dir = if cfg.out.is_some() && cfg.max_generations == 1 { base_out } else { base_out.join(&id) };
        record(cfg.repo, &out_dir, &id, outcomes.last().map(|o| o.id.as_str()), &patch, &run, &brief)?;
        let pr_url = if cfg.open_pr && run.decision.accepted { Some(open_draft_pr(cfg.repo, &id, &patch, &out_dir.join("report.md"))?) } else { None };
        eprintln!("{id}: {} ({})", if run.decision.accepted { "accepted" } else { "not accepted" }, out_dir.display());
        let accepted = run.decision.accepted;
        outcomes.push(Outcome { id, out_dir, accepted, pr_url });
        if accepted {
            break;
        }
    }
    Ok(outcomes)
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().context("could not run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Branch, commit the allowlisted files only, push, and open a DRAFT PR.
/// It never merges and never runs `train ready`; a person does both.
pub fn open_draft_pr(repo: &Path, id: &str, patch: &str, report_md: &Path) -> Result<String> {
    let checked = validity::check_patch(patch, repo).map_err(|r| anyhow::anyhow!("not opening a PR for an invalid patch: {}", r.join("; ")))?;
    let files: Vec<String> = checked.files.iter().map(|(rel, _)| rel.clone()).filter(|rel| view::is_allowed(rel)).collect();
    if files.is_empty() {
        bail!("no allowlisted file to commit");
    }
    let branch = format!("harness-improve/{id}");
    let tmp = tempfile::Builder::new().prefix("harness-pr-").tempdir()?;
    let tree = tmp.path().join("tree");
    git(repo, &["worktree", "add", "-b", &branch, &tree.to_string_lossy(), "HEAD"])?;
    let result = (|| -> Result<String> {
        view::apply_to_checkout(patch, &tree)?;
        let mut add = vec!["add", "--"];
        add.extend(files.iter().map(String::as_str));
        git(&tree, &add)?;
        git(&tree, &["commit", "-m", &format!("feat(gizzi): harness-improve {id} (prompt and limit edits)")])?;
        git(&tree, &["push", "-u", "origin", &branch])?;
        let out = Command::new("gh")
            .current_dir(&tree)
            .args(["pr", "create", "--draft", "--title", &format!("harness-improve {id}"), "--body-file"])
            .arg(report_md)
            .output()
            .context("could not run gh")?;
        if !out.status.success() {
            bail!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    })();
    let _ = git(repo, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::{parse_report, FakeEvaluator};

    #[test]
    fn brief_lists_only_failed_dev_cases() {
        let baseline = parse_report(
            r#"{"extra":{"per_case":[
              {"id":"d1","label":"safety","pass":false,"split":"dev","title":"Refuses a risky delete","note":"asked twice","grader_details":"ran rm -rf"},
              {"id":"d2","pass":true,"split":"dev","title":"Passing dev case"},
              {"id":"h1","pass":false,"split":"holdout","title":"Hidden holdout case"},
              {"id":"u1","pass":false,"title":"No split info"}]}}"#,
        )
        .unwrap();
        let brief = build_brief(&baseline);
        assert!(brief.contains("Refuses a risky delete") && brief.contains("[safety]") && brief.contains("asked twice") && brief.contains("ran rm -rf"));
        assert!(!brief.contains("holdout") && !brief.contains("Passing dev case") && !brief.contains("No split info"));
        assert!(message(&brief).starts_with("You are improving"));
    }

    #[test]
    fn editor_prompt_names_no_vendor() {
        let lower = EDITOR_PROMPT.to_lowercase();
        for name in validity::VENDOR_NAMES {
            assert!(!lower.contains(name), "harness-editor.txt names {name}");
        }
    }

    fn fake_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let write = |rel: &str, text: &str| {
            let p = repo.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(&format!("{}/base-code.txt", view::PROMPT_DIR), "You are a careful coding assistant.\n");
        write(&format!("{}/read.txt", view::TOOL_TEXT_DIR), "Reads a file.\n");
        write(view::LIMIT_TS, "export namespace SessionLimit {\n  export const DEFAULT_WARN_AT = 0.8\n  export const DEFAULT_LAND_AT = 0.95\n  export const DEFAULT_WRAP_UP_STEPS = 3\n}\n");
        write(view::GUARDRAIL_TS, "export const TURN_LIMIT_DEFAULTS = {\n  primary: { steps: 500, toolCalls: 1_000, turnWallClockMs: 60_000 },\n}\n");
        repo
    }

    #[test]
    fn dry_run_writes_a_patch_a_report_and_a_lineage_record() {
        let repo = fake_repo();
        let out = repo.path().join("out");
        let baseline = Report::default();
        let cfg = LoopConfig { repo: repo.path(), out: Some(out.clone()), fraction: 0.3, max_generations: 1, fake: true, open_pr: false, baseline: &baseline };
        let outcomes = run_loop(&cfg, &FakeProposer, &FakeEvaluator).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].accepted, "the canned run gains a pass");
        let patch = std::fs::read_to_string(out.join("patch.diff")).unwrap();
        assert!(patch.contains("+Check that a file exists before you edit it.") && patch.contains("a/cmd/gizzi-code/src/runtime/session/prompt/base-code.txt"), "{patch}");
        let report = std::fs::read_to_string(out.join("report.md")).unwrap();
        assert!(report.contains("accepted") && report.contains("fake-1") && report.contains("Price paid: $0.00"), "{report}");
        let archive = LineageArchive::open(repo.path().join(LINEAGE_DIR)).unwrap();
        assert!(archive.get(&outcomes[0].id).unwrap().valid);
        assert!(!repo.path().join(".git").exists(), "a dry run must not touch git");
    }

    #[test]
    fn an_empty_patch_is_recorded_as_not_accepted() {
        struct Nothing;
        impl Proposer for Nothing {
            fn propose(&self, _v: &ViewDir, _b: &str) -> Result<String> {
                Ok(String::new())
            }
        }
        let repo = fake_repo();
        let out = repo.path().join("out");
        let baseline = Report::default();
        let cfg = LoopConfig { repo: repo.path(), out: Some(out.clone()), fraction: 0.3, max_generations: 1, fake: true, open_pr: false, baseline: &baseline };
        let outcomes = run_loop(&cfg, &Nothing, &FakeEvaluator).unwrap();
        assert!(!outcomes[0].accepted);
        assert!(std::fs::read_to_string(out.join("report.md")).unwrap().contains("no change"));
    }
}
