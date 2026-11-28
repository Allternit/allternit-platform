//! `harness-improve`: proposes, gates and scores edits to Gizzi's prompts,
//! tool texts and turn limits, using `allternit-self-improve`.
//!
//! It runs on the person's own brain, from a platform checkout. It never
//! merges and never marks anything train-ready. `--open-pr` (off by default)
//! is the only way it touches a remote, and that PR is a draft.
//!
//! `--dry-run` (or env `HARNESS_IMPROVE_FAKE=1`) swaps in a fake proposer and
//! a fake evaluator, so the whole flow runs with no network and no price.

mod evaluator;
mod proposer;
mod validity;
mod view;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "harness-improve", version, about = "Propose, gate and score edits to Gizzi's prompts and turn limits. Never merges.")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,

    #[command(flatten)]
    opts: Opts,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Ask the brain for one candidate patch to the harness view.
    Propose,
    /// Check one patch: validity, then the staged eval, then the full eval.
    Gate {
        /// A repo-relative unified diff (as `git diff` writes it).
        #[arg(long)]
        patch: PathBuf,
    },
    /// Propose and gate, for up to --max-generations rounds.
    Run,
}

#[derive(Debug, Clone, clap::Args, Serialize)]
struct Opts {
    /// The platform repo root. Defaults to the current folder.
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
    /// The protected eval suite folder.
    #[arg(long, global = true)]
    suite_dir: Option<PathBuf>,
    /// How many propose-and-gate rounds `run` may do.
    #[arg(long, global = true, default_value_t = 1)]
    max_generations: u32,
    /// Hard stop on spend, in US dollars.
    #[arg(long, global = true)]
    max_usd: Option<f64>,
    /// Share of the dev cases the staged eval runs (0 < f <= 1).
    #[arg(long, global = true, default_value_t = 0.3)]
    fraction: f64,
    /// Seed for case sampling and parent choice.
    #[arg(long, global = true)]
    seed: Option<u64>,
    /// Do the local steps only: no brain call, no eval, no child process.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Skip the price prompt.
    #[arg(long, global = true)]
    yes: bool,
    /// The baseline eval report (`allternit.eval_report.v1`) to score against.
    #[arg(long, global = true)]
    baseline: Option<PathBuf>,
    /// Where patch.diff and report.md go. Default: .harness-improve/out/<id> in the repo.
    #[arg(long, global = true)]
    out: Option<PathBuf>,
    /// Open a draft PR with an accepted patch. Off by default.
    #[arg(long, global = true)]
    open_pr: bool,
    /// The brain (runtime) that proposes and runs the eval.
    #[arg(long, global = true)]
    brain: Option<String>,
    /// The model on that brain.
    #[arg(long, global = true)]
    model: Option<String>,
}

impl Opts {
    fn validate(&self) -> Result<()> {
        if !(self.fraction > 0.0 && self.fraction <= 1.0) {
            bail!("--fraction must be above 0 and at most 1 (got {})", self.fraction);
        }
        if self.max_generations == 0 {
            bail!("--max-generations must be at least 1");
        }
        if let Some(usd) = self.max_usd {
            if !(usd.is_finite() && usd > 0.0) {
                bail!("--max-usd must be a positive amount (got {usd})");
            }
        }
        if self.open_pr && self.dry_run {
            bail!("--open-pr does nothing on a dry run; drop one of them");
        }
        Ok(())
    }

    fn repo(&self) -> Result<PathBuf> {
        let repo = match &self.repo {
            Some(path) => path.clone(),
            None => std::env::current_dir().context("could not read the current folder")?,
        };
        if !repo.join(view::LIMIT_TS).is_file() {
            bail!("{} is not the platform repo root (no {}). Run from the repo root or pass --repo.", repo.display(), view::LIMIT_TS);
        }
        Ok(repo)
    }
}

/// Fake proposer and evaluator: `--dry-run`, or env `HARNESS_IMPROVE_FAKE=1`.
fn is_fake(opts: &Opts) -> bool {
    opts.dry_run || std::env::var("HARNESS_IMPROVE_FAKE").map(|v| v == "1").unwrap_or(false)
}

/// The evaluator: the candidate's Gizzi runs the protected suite.
fn make_evaluator(opts: &Opts, repo: &Path, baseline: Option<&evaluator::Report>) -> Result<Box<dyn evaluator::ScoredEvaluator>> {
    if is_fake(opts) {
        return Ok(Box::new(evaluator::FakeEvaluator));
    }
    let Some(baseline) = baseline else { bail!("--baseline <report.json> is required: the first live baseline is made by hand") };
    let suite_dir = opts.suite_dir.clone().unwrap_or_else(|| repo.join("domains/agent-evals/gizzi/suites/shared.v1"));
    Ok(Box::new(evaluator::GizziEvaluator::new(evaluator::EvalSettings {
        launcher: evaluator::Launcher::Dev,
        suite_dir,
        baseline: baseline.clone(),
        seed: opts.seed.unwrap_or(0),
        model: opts.model.clone(),
        yes: opts.yes,
        max_usd: opts.max_usd,
    })))
}

fn make_proposer(opts: &Opts) -> Box<dyn proposer::Proposer> {
    if is_fake(opts) {
        Box::new(proposer::FakeProposer)
    } else {
        Box::new(proposer::GizziProposer { model: opts.model.clone() })
    }
}

fn load_baseline(opts: &Opts) -> Result<Option<evaluator::Report>> {
    match &opts.baseline {
        Some(path) => Ok(Some(evaluator::load_report(path)?)),
        None => Ok(None),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    cli.opts.validate()?;
    match &cli.command {
        Cmd::Propose => propose(&cli.opts),
        Cmd::Gate { patch } => gate(&cli.opts, patch),
        Cmd::Run => run(&cli.opts),
    }
}

fn print_settings(opts: &Opts) -> Result<()> {
    eprintln!("settings: {}", serde_json::to_string(opts)?);
    Ok(())
}

fn propose(opts: &Opts) -> Result<()> {
    let repo = opts.repo()?;
    let view = view::export_view(&repo)?;
    eprintln!("harness view: {} editable files in {}", view.files().len(), view.root().display());
    if opts.dry_run {
        print_settings(opts)?;
    }
    let baseline = load_baseline(opts)?.unwrap_or_default();
    let patch = make_proposer(opts).propose(&view, &proposer::build_brief(&baseline))?;
    print!("{patch}");
    Ok(())
}

fn gate(opts: &Opts, patch_file: &Path) -> Result<()> {
    let repo = opts.repo()?;
    let patch = std::fs::read_to_string(patch_file).with_context(|| format!("could not read {}", patch_file.display()))?;

    // Static checks first: they need no child process.
    if let Err(reasons) = validity::check_patch(&patch, &repo) {
        for reason in &reasons {
            eprintln!("rejected: {reason}");
        }
        bail!("the patch is not valid ({} problem{})", reasons.len(), if reasons.len() == 1 { "" } else { "s" });
    }
    eprintln!("static checks passed: allowlist, numbers-only, bounds, vendor names, byte caps");
    if opts.dry_run {
        print_settings(opts)?;
        eprintln!("dry run: skipping the guard tests and the eval");
        return Ok(());
    }

    // Fail before making a checkout if the evaluator cannot be made.
    let baseline = load_baseline(opts)?;
    let evaluator = make_evaluator(opts, &repo, baseline.as_ref())?;
    let run = proposer::gate_patch(&repo, &patch, evaluator.as_ref(), is_fake(opts), opts.fraction)?;
    for reason in &run.rejections {
        eprintln!("rejected: {reason}");
    }
    let id = format!("gate-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let out_dir = opts.out.clone().unwrap_or_else(|| repo.join(proposer::OUT_DIR).join(&id));
    proposer::record(&repo, &out_dir, &id, None, &patch, &run, "(patch given with --patch)")?;
    eprintln!("wrote {}", out_dir.display());
    println!("{}", serde_json::to_string_pretty(&run.report)?);
    if !run.decision.accepted {
        for reason in &run.decision.reasons {
            eprintln!("not accepted: {reason}");
        }
        bail!("the patch was not accepted");
    }
    Ok(())
}

fn run(opts: &Opts) -> Result<()> {
    let repo = opts.repo()?;
    if opts.dry_run {
        print_settings(opts)?;
    }
    if opts.open_pr && is_fake(opts) {
        bail!("--open-pr cannot be used with HARNESS_IMPROVE_FAKE=1");
    }
    let baseline = load_baseline(opts)?;
    let evaluator = make_evaluator(opts, &repo, baseline.as_ref())?;
    let baseline_report = baseline.clone().unwrap_or_default();
    let cfg = proposer::LoopConfig {
        repo: &repo,
        out: opts.out.clone(),
        fraction: opts.fraction,
        max_generations: opts.max_generations,
        fake: is_fake(opts),
        open_pr: opts.open_pr,
        baseline: &baseline_report,
    };
    let outcomes = proposer::run_loop(&cfg, make_proposer(opts).as_ref(), evaluator.as_ref())?;
    for o in &outcomes {
        if let Some(url) = &o.pr_url {
            println!("draft PR: {url}");
        }
    }
    if !outcomes.iter().any(|o| o.accepted) {
        eprintln!("no patch was accepted; see report.md in each output folder");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("harness-improve").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn defaults_are_one_generation_a_third_of_the_cases_and_no_pr() {
        let cli = parse(&["run"]);
        assert_eq!(cli.opts.max_generations, 1);
        assert_eq!(cli.opts.fraction, 0.3);
        assert!(!cli.opts.open_pr && !cli.opts.dry_run && !cli.opts.yes);
        cli.opts.validate().unwrap();
    }

    #[test]
    fn gate_requires_a_patch_file() {
        assert!(Cli::try_parse_from(["harness-improve", "gate"]).is_err());
        let cli = parse(&["gate", "--patch", "x.patch", "--dry-run"]);
        assert!(matches!(cli.command, Cmd::Gate { ref patch } if patch == Path::new("x.patch")));
        assert!(cli.opts.dry_run);
    }

    #[test]
    fn bad_flag_values_are_refused() {
        assert!(parse(&["run", "--fraction", "0"]).opts.validate().is_err());
        assert!(parse(&["run", "--fraction", "1.5"]).opts.validate().is_err());
        assert!(parse(&["run", "--max-generations", "0"]).opts.validate().is_err());
        assert!(parse(&["run", "--max-usd", "-1"]).opts.validate().is_err());
        assert!(parse(&["run", "--dry-run", "--open-pr"]).opts.validate().is_err());
        assert!(parse(&["run", "--max-usd", "2.5", "--seed", "7", "--brain", "local", "--model", "m"]).opts.validate().is_ok());
    }
}
