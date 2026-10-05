//! `allternit-factory internal rails observe` — run the read-only observer once.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Args;

use crate::ledger::Ledger;
use crate::observer::{observe, ObserveRequest, ObserverConfig, Trigger};

#[derive(Debug, Args)]
pub struct ObserveArgs {
    /// DAG to observe (message goes to `dag:<id>` unless --wih is given).
    #[arg(long = "dag")]
    pub dag_id: String,
    /// Focus WIH (message goes to `wih:<id>`).
    #[arg(long = "wih")]
    pub wih_id: Option<String>,
    /// plan | repeat-failure | pre-close
    #[arg(long)]
    pub trigger: Trigger,
    /// Consult command override (else observer.json `consult_cmd`,
    /// `ALLTERNIT_OBSERVER_CMD`, then `STEER_CONSULT_CMD`). Always run through
    /// a read-only profile.
    #[arg(long = "consult-cmd")]
    pub consult_cmd: Option<String>,
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run_observe_command(
    root: &Path,
    ledger: Arc<Ledger>,
    args: ObserveArgs,
) -> Result<()> {
    let cfg = ObserverConfig::load(root)?;
    let cmd = args
        .consult_cmd
        .clone()
        .or_else(|| cfg.explicit_consult_cmd())
        .ok_or_else(|| {
            anyhow!(
                "no observer consult command: set consult_cmd in .allternit/rails/observer.json, \
                 ALLTERNIT_OBSERVER_CMD, or pass --consult-cmd"
            )
        })?;
    let req = ObserveRequest {
        dag_id: args.dag_id,
        wih_id: args.wih_id,
        trigger: args.trigger,
        detail: None,
        subject_tag: None,
    };
    let out = observe(root, ledger, &cfg, &cmd, &req).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("thread_id: {}", out.thread_id);
        println!("message_id: {}", out.message_id);
        println!("profile: {}", out.profile);
        println!("--- advice ---\n{}", out.advice);
    }
    Ok(())
}
