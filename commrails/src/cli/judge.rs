//! `judge …` and `leases …` subcommands (spec: `spec/JUDGE.md`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use clap::Subcommand;
use serde_json::json;

use crate::core::types::{Actor, ActorType};
use crate::gate::gate::HumanDecision;
use crate::judge::config::{config_path, load_config};
use crate::judge::heartbeat::parse_duration;
use crate::judge::policy::{CloseBy, Fence, JudgePolicy, PolicyOrigin, VerifyMode};
use crate::judge::state::{pending_judge_needs, project_node_judge};
use crate::{Gate, Ledger, LedgerQuery};

#[derive(Subcommand)]
pub enum JudgeCmd {
    /// Judge policy (opt-in per plan or node; default off).
    #[command(subcommand)]
    Policy(JudgePolicyCmd),
    /// allow | ask | deny for one tool call (Gate 2 denials and the hard
    /// floor are final; a judge failure is `ask`).
    Tool {
        #[arg(long)]
        wih: String,
        #[arg(long)]
        tool: String,
        /// The command line (Bash-style tools).
        #[arg(long)]
        command: Option<String>,
        /// Paths the call touches.
        #[arg(long, num_args = 0..)]
        paths: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Policy, verdict history and continuation count for a node.
    Show {
        /// `<dag_id>/<node_id>`
        node: String,
    },
    /// Re-open a node in EXCEPTION (counts against max_continuations).
    Continue {
        /// `<dag_id>/<node_id>`
        node: String,
        /// `user:<id>` or `agent:<id>` (bare id = user).
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// A person resolves a node in EXCEPTION / NEEDS_HUMAN.
    Resolve {
        /// `<dag_id>/<node_id>`
        node: String,
        decision: HumanDecision,
        /// Must be a user: `user:<id>` (bare id = user).
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Nodes waiting on a person because of a verdict.
    Pending {
        #[arg(long)]
        json: bool,
    },
    /// Show the judge backend config (`.allternit/judge/config.json`).
    Config,
}

#[derive(Subcommand)]
pub enum JudgePolicyCmd {
    Set {
        #[arg(long = "dag")]
        dag_id: String,
        /// Node-level policy (overrides the plan level).
        #[arg(long = "node")]
        node_id: Option<String>,
        #[arg(long)]
        verify: Option<VerifyMode>,
        #[arg(long = "close-by")]
        close_by: Option<CloseBy>,
        #[arg(long = "tool-judge")]
        tool_judge: Option<bool>,
        #[arg(long = "max-continuations")]
        max_continuations: Option<u32>,
        /// Origin marker; forces verifier-owned completion and cannot be unset.
        #[arg(long)]
        origin: Option<PolicyOrigin>,
        /// Completion policy id, e.g. `completion.bug_fix`.
        #[arg(long = "completion-policy")]
        completion_policy: Option<String>,
        /// Fence profile (Q25): `guardrail` (default) or opt-in `strict`.
        #[arg(long)]
        fence: Option<Fence>,
        /// `user:<id>` or `agent:<id>` (bare id = user).
        #[arg(long)]
        actor: String,
    },
    Show {
        #[arg(long = "dag")]
        dag_id: String,
        #[arg(long = "node")]
        node_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum LeasesCmd {
    /// Heartbeat for the holder of a WIH (the drive runner calls this ~60s).
    Heartbeat {
        wih_id: String,
        #[arg(long)]
        pid: Option<u32>,
        #[arg(long)]
        host: Option<String>,
    },
    /// Release leases of stale holders; close their open WIHs as RECLAIMED.
    Reclaim {
        /// e.g. 300, 300s, 5m, 1h
        #[arg(long = "stale-after", default_value = "5m")]
        stale_after: String,
        /// Also reclaim leases whose WIH never heartbeated (by lease age).
        #[arg(long)]
        include_unbeaten: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}

pub struct JudgeContext {
    pub root: PathBuf,
    pub ledger: Arc<Ledger>,
    pub gate: Arc<Gate>,
}

/// `user:<id>` | `agent:<id>` | bare id (= user).
pub fn parse_actor(raw: &str) -> Result<Actor> {
    let (kind, id) = match raw.split_once(':') {
        Some(("user", id)) => (ActorType::User, id),
        Some(("agent", id)) => (ActorType::Agent, id),
        Some((other, _)) => bail!("--actor type must be user or agent, got {other:?}"),
        None => (ActorType::User, raw),
    };
    if id.trim().is_empty() {
        bail!("--actor needs an id");
    }
    Ok(Actor {
        r#type: kind,
        id: id.to_string(),
    })
}

fn split_node(node: &str) -> Result<(String, String)> {
    node.split_once('/')
        .filter(|(d, n)| !d.is_empty() && !n.is_empty())
        .map(|(d, n)| (d.to_string(), n.to_string()))
        .ok_or_else(|| anyhow!("node must be <dag_id>/<node_id>, got {node:?}"))
}

pub async fn run_judge_command(ctx: &JudgeContext, cmd: JudgeCmd) -> Result<()> {
    match cmd {
        JudgeCmd::Policy(JudgePolicyCmd::Set {
            dag_id,
            node_id,
            verify,
            close_by,
            tool_judge,
            max_continuations,
            origin,
            completion_policy,
            fence,
            actor,
        }) => {
            let actor = parse_actor(&actor)?;
            let policy = JudgePolicy {
                verify,
                close_by,
                tool_judge,
                max_continuations,
                origin,
                completion_policy,
                fence,
            };
            let eff = ctx
                .gate
                .set_judge_policy(&dag_id, node_id.as_deref(), policy, &actor)
                .await?;
            println!("{}", serde_json::to_string_pretty(&eff)?);
        }
        JudgeCmd::Policy(JudgePolicyCmd::Show { dag_id, node_id }) => {
            let eff = ctx.gate.judge_policy(&dag_id, node_id.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&eff)?);
        }
        JudgeCmd::Tool {
            wih,
            tool,
            command,
            paths,
            json: as_json,
        } => {
            let v = ctx
                .gate
                .judge_tool_call(&wih, &tool, command.as_deref(), &paths)
                .await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("decision: {}", v.decision.as_str());
                println!(
                    "source: {}",
                    serde_json::to_value(v.source)?.as_str().unwrap_or("")
                );
                println!("reason: {}", v.reason);
            }
        }
        JudgeCmd::Show { node } => {
            let (dag_id, node_id) = split_node(&node)?;
            let events = ctx.ledger.query(LedgerQuery::default()).await?;
            let policy = ctx.gate.judge_policy(&dag_id, Some(&node_id)).await?;
            let st = project_node_judge(&events, &dag_id, &node_id);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "dag_id": dag_id,
                    "node_id": node_id,
                    "policy": policy,
                    "continuations_used": st.continuations_used,
                    "verdicts": st.verdicts,
                }))?
            );
        }
        JudgeCmd::Continue {
            node,
            actor,
            reason,
        } => {
            let (dag_id, node_id) = split_node(&node)?;
            let actor = parse_actor(&actor)?;
            let n = ctx
                .gate
                .judge_continue(&dag_id, &node_id, &actor, reason.as_deref())
                .await?;
            println!("continued: {dag_id}/{node_id} (continuation {n}) -> READY");
        }
        JudgeCmd::Resolve {
            node,
            decision,
            actor,
            reason,
        } => {
            let (dag_id, node_id) = split_node(&node)?;
            let actor = parse_actor(&actor)?;
            let to = ctx
                .gate
                .judge_resolve(&dag_id, &node_id, decision, &actor, reason.as_deref())
                .await?;
            println!("resolved: {dag_id}/{node_id} -> {to}");
        }
        JudgeCmd::Pending { json: as_json } => {
            let events = ctx.ledger.query(LedgerQuery::default()).await?;
            let pending = pending_judge_needs(&events);
            if as_json {
                println!("{}", serde_json::to_string_pretty(&pending)?);
            } else {
                for p in pending {
                    println!(
                        "{} {} [{}] {}{} - {}",
                        p.dag_id,
                        p.node_id,
                        p.reason,
                        p.node_title,
                        p.category.map(|c| format!(" ({c})")).unwrap_or_default(),
                        p.detail
                    );
                }
            }
        }
        JudgeCmd::Config => {
            let path = config_path(&ctx.root);
            match load_config(&ctx.root) {
                Ok(cfg) => {
                    println!(
                        "# {} ({})",
                        path.display(),
                        if path.exists() {
                            "file"
                        } else {
                            "absent: defaults"
                        }
                    );
                    println!("{}", serde_json::to_string_pretty(&cfg)?);
                }
                Err(err) => {
                    println!(
                        "# {}: INVALID — every verdict is needs_human, every tool call ask",
                        path.display()
                    );
                    println!("{err}");
                }
            }
        }
    }
    Ok(())
}

pub async fn run_leases_command(ctx: &JudgeContext, cmd: LeasesCmd) -> Result<()> {
    match cmd {
        LeasesCmd::Heartbeat { wih_id, pid, host } => {
            let beat = ctx.gate.lease_heartbeat(&wih_id, pid, host).await?;
            println!("{}", serde_json::to_string(&beat)?);
        }
        LeasesCmd::Reclaim {
            stale_after,
            include_unbeaten,
            dry_run,
            json: as_json,
        } => {
            let stale_after = parse_duration(&stale_after)?;
            let recs = ctx
                .gate
                .reclaim_stale_leases(stale_after, include_unbeaten, dry_run)
                .await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&recs)?);
            } else if recs.is_empty() {
                println!("no stale holders");
            } else {
                for r in recs {
                    println!(
                        "{}{} {} leases={:?} wih_reclaimed={} - {}",
                        if r.dry_run { "(dry-run) " } else { "" },
                        r.wih_id,
                        r.node.unwrap_or_default(),
                        r.lease_ids,
                        r.wih_reclaimed,
                        r.reason
                    );
                }
            }
        }
    }
    Ok(())
}
