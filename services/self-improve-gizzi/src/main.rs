//! `harness-improve`: proposes, gates and scores edits to Gizzi's prompts,
//! tool texts and turn limits, using `allternit-self-improve`.
//!
//! It runs on the person's own brain, from a platform checkout. It never
//! merges and never marks anything train-ready. `--open-pr` (off by default)
//! is the only way it touches a remote, and that PR is a draft.
//!
//! Built in steps: this one has the CLI, the harness view and the validity
//! check. The evaluator (step 3.3) and the proposer (step 3.4) are stubs that
//! return a clear "not built yet" error.

mod validity;
mod view;

use allternit_self_improve::eval::{Evaluator, StagedGate};
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

/// The evaluator: the candidate's Gizzi runs the protected suite. Step 3.3.
fn make_evaluator(_opts: &Opts) -> Result<Box<dyn Evaluator>> {
    bail!("the evaluator is not built yet (step 3.3)")
}

/// The proposer: one patch from the person's brain. Step 3.4.
fn propose_patch(_opts: &Opts, _view: &view::ViewDir) -> Result<String> {
    bail!("the proposer is not built yet (step 3.4)")
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
    let patch = propose_patch(opts, &view)?;
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

    // Fail before making a checkout if the evaluator is missing.
    let evaluator = make_evaluator(opts)?;
    let checkout = view::CandidateCheckout::create(&repo)?;
    view::apply_to_checkout(&patch, checkout.path())?;
    let validity = validity::HarnessValidity::new(&repo, patch, Box::new(validity::BunGuardTests));
    let report = StagedGate::new(&validity, evaluator.as_ref(), opts.fraction).run(checkout.path())?;
    for reason in validity.rejections() {
        eprintln!("rejected: {reason}");
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.valid {
        bail!("the patch did not pass the gate");
    }
    Ok(())
}

fn run(opts: &Opts) -> Result<()> {
    let repo = opts.repo()?;
    let view = view::export_view(&repo)?;
    eprintln!("harness view: {} editable files in {}", view.files().len(), view.root().display());
    if opts.dry_run {
        print_settings(opts)?;
    }
    // Step 3.4 fills this loop: propose, gate, record lineage, write the
    // patch and report.md, and open a draft PR only with --open-pr.
    let _evaluator = make_evaluator(opts)?;
    let _patch = propose_patch(opts, &view)?;
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

    #[test]
    fn the_unbuilt_steps_say_so() {
        let opts = parse(&["run"]).opts;
        let err = make_evaluator(&opts).err().unwrap().to_string();
        assert!(err.contains("step 3.3"), "{err}");
    }
}
