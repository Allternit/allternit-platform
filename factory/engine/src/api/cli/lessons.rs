//! `allternit-factory internal rails lessons …` — vault memory candidates and triage.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Subcommand;

use crate::ledger::Ledger;
use crate::lessons::sink::{MemorySink, VaultCandidateSink};
use crate::lessons::triage::{
    default_brain_root, report_applied_outcomes, report_rejected_outcomes, triage_dag, TriageConfig, DEFAULT_MEAN_MIN, DEFAULT_SYSTEM_ONE_MODEL,
    DEFAULT_SYSTEM_ONE_URL, DEFAULT_TASK_MIN,
};

#[derive(Debug, Subcommand)]
pub enum LessonsCmd {
    /// List vault memory candidates (pending human approval).
    List {
        #[arg(long = "dag")]
        dag_id: Option<String>,
    },
    /// Score a DAG's candidates with three System One Nouls and write
    /// promoted (or unscored) ones as Brain drafts (confirm:false).
    Triage {
        #[arg(long = "dag")]
        dag_id: String,
        /// `Allternit Brain/` root; drafts go to `<root>/.incoming/`.
        /// Default: $ALLTERNIT_BRAIN_ROOT or ~/Desktop/Allternit/Allternit Brain.
        #[arg(long = "brain-root")]
        brain_root: Option<PathBuf>,
        /// S1 decision runtime base URL (`/v1/decision`).
        #[arg(long, default_value = DEFAULT_SYSTEM_ONE_URL)]
        server: String,
        /// Recorded on the LessonTriaged event only; the runtime picks the
        /// model via `ALLTERNIT_S1_BACKEND` (default `auto`).
        #[arg(long, default_value = DEFAULT_SYSTEM_ONE_MODEL)]
        model: String,
        /// Promote only when task_success >= this.
        #[arg(long = "task-min", default_value_t = DEFAULT_TASK_MIN)]
        task_min: f64,
        /// ...and the mean of the three Nouls >= this.
        #[arg(long = "mean-min", default_value_t = DEFAULT_MEAN_MIN)]
        mean_min: f64,
        #[arg(long = "timeout-secs", default_value_t = 30)]
        timeout_secs: u64,
        /// Re-triage candidates that were already triaged.
        #[arg(long)]
        force: bool,
    },
    /// Report S1 outcome labels for triage drafts a human applied
    /// (`<brain-root>/.incoming/applied/`, true) or rejected
    /// (`.incoming/rejected/`, false per `x_rejection.why`). Each candidate is
    /// labelled once.
    Outcomes {
        #[arg(long = "brain-root")]
        brain_root: Option<PathBuf>,
        /// S1 decision runtime base URL.
        #[arg(long, default_value = DEFAULT_SYSTEM_ONE_URL)]
        server: String,
    },
}

pub async fn run_lessons_command(root: &Path, ledger: Arc<Ledger>, cmd: LessonsCmd) -> Result<()> {
    let sink = VaultCandidateSink::new(root);
    match cmd {
        LessonsCmd::Outcomes { brain_root, server } => {
            let reporter = crate::kernel::s1_outcome::OutcomeReporter::for_url(&server, Duration::from_secs(5));
            let brain_root = brain_root.unwrap_or_else(default_brain_root);
            let n = report_applied_outcomes(&ledger, &brain_root, &reporter).await?;
            let r = report_rejected_outcomes(&ledger, &brain_root, &reporter).await?;
            println!("labelled {n} applied and {r} rejected lesson draft(s)");
        }
        LessonsCmd::List { dag_id } => {
            let list = sink.list(dag_id.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&list)?);
        }
        LessonsCmd::Triage {
            dag_id,
            brain_root,
            server,
            model,
            task_min,
            mean_min,
            timeout_secs,
            force,
        } => {
            let mut cfg = TriageConfig::new(brain_root.unwrap_or_else(default_brain_root));
            cfg.server_url = server;
            cfg.model = model;
            cfg.task_min = task_min;
            cfg.mean_min = mean_min;
            cfg.timeout = Duration::from_secs(timeout_secs.max(1));
            cfg.force = force;
            let results = triage_dag(&ledger, &sink, &cfg, &dag_id).await?;
            if results.is_empty() {
                println!("no untriaged memory candidates for dag {dag_id}");
            }
            for r in &results {
                let draft = r
                    .draft_path
                    .as_ref()
                    .map(|d| format!(" draft={d}"))
                    .unwrap_or_default();
                match &r.scores {
                    Some(s) => println!(
                        "{} {} task_success={:.2} reusable={:.2} supported={:.2} mean={:.2}{draft}",
                        r.candidate_id,
                        r.verdict.as_str(),
                        s.task_success,
                        s.reusable_pattern,
                        s.supported_by_events,
                        s.mean(),
                    ),
                    None => println!(
                        "{} unscored ({}){draft}",
                        r.candidate_id,
                        r.unscored_reason.as_deref().unwrap_or("?"),
                    ),
                }
            }
        }
    }
    Ok(())
}
