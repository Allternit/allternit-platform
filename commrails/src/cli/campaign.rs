//! CLI: `campaign …`, `wake …`, `attention …`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use clap::Subcommand;

use crate::attention::{AttentionChannel, AttentionGate, AttentionRequest, ItemState};
use crate::campaign::{
    BudgetDecl, BudgetMode, Campaign, CampaignDefinition, CampaignOps, CampaignStatus, Rearm,
    SpendEntry,
};
use crate::core::types::{Actor, ActorType};
use crate::gate::Gate;
use crate::ledger::Ledger;
use crate::wake::runner::{run_due, NodeTimerHandler, SweepContext};
use crate::wake::{project_unfinished, AutomationConfig, WakeQueue};

pub struct AutomationCliContext {
    pub root: PathBuf,
    pub ledger: Arc<Ledger>,
}

#[derive(Subcommand)]
pub enum CampaignCmd {
    /// Declare a campaign from flags or a definition file (YAML/JSON).
    Declare {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        objective: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        /// `bot:<slug>` | `ao:<harness>` | `command`
        #[arg(long)]
        executor: Option<String>,
        /// Shell command for `--executor command`.
        #[arg(long)]
        command: Option<String>,
        /// Declared budget unit (not interpreted).
        #[arg(long)]
        budget_unit: Option<String>,
        #[arg(long)]
        budget_limit: Option<f64>,
        #[arg(long, default_value = "additive")]
        budget_mode: BudgetMode,
        /// Spend recorded automatically per fired check.
        #[arg(long)]
        per_wake: Option<f64>,
        #[arg(long = "dag")]
        dag_id: Option<String>,
        /// Re-arm the check every N seconds after it fires.
        #[arg(long)]
        rearm_every_secs: Option<i64>,
        /// Declare paused instead of active.
        #[arg(long)]
        paused: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Status {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Arm the campaign's one pending check (replaces any pending one).
    /// Delay is clamped to [60s, campaign.check_ceiling_secs (default 7d)].
    CheckLater {
        id: String,
        delay_secs: i64,
        #[arg(long)]
        message: Option<String>,
    },
    Note {
        id: String,
        text: String,
    },
    /// Record spend. Exhausting the budget pauses the campaign and raises a
    /// needs-you item. Budgets do not cap provider bills.
    Spend {
        id: String,
        amount: f64,
        /// Span start (RFC 3339); shared budgets overlap spans per resource.
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long)]
        resource: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    Pause {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Resume a paused campaign; `--limit` raises an exhausted budget.
    Resume {
        id: String,
        #[arg(long)]
        limit: Option<f64>,
    },
    Kill {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Finish {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum WakeCmd {
    /// Pending wakes (one per key) and wakes a dead sweep left unfinished.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Pending wakes due now (or at `--at`).
    Due {
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Fire due wakes once under the sweep lock. Exits 0 with "locked" when
    /// another sweep holds the lock. `--at` evaluates due-ness at that
    /// instant (replay/testing).
    RunDue {
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Cancel the pending wake for a key (`campaign:<id>`, `node:<dag>/<node>`).
    Cancel {
        key: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum AttentionCmd {
    /// Items through the attention gate (`--open`: delivered, un-acked).
    List {
        #[arg(long)]
        open: bool,
        #[arg(long)]
        json: bool,
    },
    /// Submit a notification (e.g. a sweep report) through the gate.
    Submit {
        #[arg(long)]
        key: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: String,
        #[arg(long, default_value = "needs-you")]
        channel: ChannelArg,
        #[arg(long, default_value = "cli")]
        source: String,
    },
    /// Release queued items whose quiet-hours/cap deferral has passed.
    Release {
        #[arg(long)]
        at: Option<String>,
    },
    /// Clear a delivered item from needs-you.
    Ack {
        item_id: String,
        #[arg(long, default_value = "user:cli")]
        by: String,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ChannelArg {
    NeedsYou,
    Mail,
}

fn at_or_now(at: Option<&str>) -> Result<DateTime<Utc>> {
    match at {
        Some(s) => Ok(s
            .parse::<DateTime<Utc>>()
            .map_err(|e| anyhow::anyhow!("--at {s:?}: {e}"))?),
        None => Ok(Utc::now()),
    }
}

fn parse_ts(label: &str, s: Option<String>) -> Result<Option<DateTime<Utc>>> {
    s.map(|v| {
        v.parse::<DateTime<Utc>>()
            .map_err(|e| anyhow::anyhow!("--{label} {v:?}: {e}"))
    })
    .transpose()
}

fn ops(ctx: &AutomationCliContext, cfg: &AutomationConfig) -> CampaignOps {
    CampaignOps::new(
        &ctx.root,
        ctx.ledger.clone(),
        cfg.campaign.check_ceiling_secs,
        cfg.attention.clone(),
    )
    .with_actor(Actor {
        r#type: ActorType::User,
        id: "cli".to_string(),
    })
}

fn print_campaign(c: &Campaign) {
    let budget = c
        .budget
        .as_ref()
        .map(|b| format!("{}/{} {} ({})", b.spent, b.limit, b.unit, b.mode))
        .unwrap_or_else(|| "none".into());
    let check = c
        .pending_check
        .as_ref()
        .map(|p| format!("{} ({})", p.due_at.to_rfc3339(), p.wake_id))
        .unwrap_or_else(|| "none".into());
    println!(
        "{} [{}{}] owner={} executor={} budget={} next_check={}",
        c.campaign_id,
        c.status,
        c.status_reason
            .as_deref()
            .map(|r| format!(": {r}"))
            .unwrap_or_default(),
        c.owner,
        c.executor.label(),
        budget,
        check
    );
    println!("  objective: {}", c.objective);
}

pub async fn run_campaign_command(ctx: &AutomationCliContext, cmd: CampaignCmd) -> Result<()> {
    let cfg = AutomationConfig::load(&ctx.root)?;
    let ops = ops(ctx, &cfg);
    let now = Utc::now();
    match cmd {
        CampaignCmd::Declare {
            file,
            id,
            objective,
            owner,
            executor,
            command,
            budget_unit,
            budget_limit,
            budget_mode,
            per_wake,
            dag_id,
            rearm_every_secs,
            paused,
        } => {
            let def = if let Some(path) = file {
                if id.is_some() || objective.is_some() || executor.is_some() {
                    bail!("--file cannot be combined with --id/--objective/--executor");
                }
                CampaignDefinition::load(&path)?
            } else {
                let (Some(id), Some(objective), Some(executor)) = (id, objective, executor) else {
                    bail!("campaign declare needs --file, or --id --objective --executor");
                };
                let budget = match (budget_unit, budget_limit) {
                    (Some(unit), Some(limit)) => Some(BudgetDecl {
                        unit,
                        limit,
                        mode: budget_mode,
                        per_wake,
                    }),
                    (None, None) => None,
                    _ => bail!("a budget needs both --budget-unit and --budget-limit"),
                };
                CampaignDefinition {
                    id,
                    objective,
                    owner: owner.unwrap_or_else(|| "eoj".to_string()),
                    status: if paused {
                        CampaignStatus::Paused
                    } else {
                        CampaignStatus::Active
                    },
                    executor,
                    command,
                    budget,
                    dag_id,
                    rearm: rearm_every_secs.map(|s| Rearm {
                        every_secs: Some(s),
                        weekdays: Vec::new(),
                        at: None,
                        timezone: None,
                    }),
                }
            };
            let c = ops.declare(def, now).await?;
            println!("declared");
            print_campaign(&c);
        }
        CampaignCmd::List { json } => {
            let all = ops.all().await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&all.values().collect::<Vec<_>>())?
                );
            } else {
                for c in all.values() {
                    print_campaign(c);
                }
            }
        }
        CampaignCmd::Status { id, json } => {
            let c = ops.get(&id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&c)?);
            } else {
                print_campaign(&c);
                for n in &c.notes {
                    println!("  note {} {}: {}", n.at, n.by, n.text);
                }
            }
        }
        CampaignCmd::CheckLater {
            id,
            delay_secs,
            message,
        } => {
            let armed = ops
                .check_later(&id, delay_secs, message.as_deref(), now)
                .await?;
            print!(
                "check armed for {} ({})",
                armed.wake.due_at.to_rfc3339(),
                armed.wake.wake_id
            );
            if let Some(c) = armed.clamped {
                print!("; delay clamped to the {c}");
            }
            if let Some(r) = armed.replaced {
                print!(
                    "; replaced pending check {} — one check is pending",
                    r.wake_id
                );
            }
            println!();
        }
        CampaignCmd::Note { id, text } => {
            ops.note(&id, &text).await?;
            println!("noted");
        }
        CampaignCmd::Spend {
            id,
            amount,
            start,
            end,
            resource,
            note,
        } => {
            let out = ops
                .spend(
                    &id,
                    SpendEntry {
                        amount,
                        start: parse_ts("start", start)?,
                        end: parse_ts("end", end)?,
                        resource,
                        note,
                    },
                    now,
                )
                .await?;
            if let Some(b) = &out.campaign.budget {
                println!("spent {}/{} {} ({})", b.spent, b.limit, b.unit, b.mode);
            } else {
                println!("recorded (campaign has no budget)");
            }
            if out.paused_for_budget {
                println!("budget exhausted: campaign paused, needs-you item raised");
            }
        }
        CampaignCmd::Pause { id, reason } => {
            print_campaign(&ops.pause(&id, reason.as_deref()).await?);
        }
        CampaignCmd::Resume { id, limit } => {
            print_campaign(&ops.resume(&id, limit, now).await?);
        }
        CampaignCmd::Kill { id, reason } => {
            print_campaign(&ops.kill(&id, reason.as_deref()).await?);
        }
        CampaignCmd::Finish { id, reason } => {
            print_campaign(&ops.finish(&id, reason.as_deref()).await?);
        }
    }
    Ok(())
}

pub async fn run_wake_command(
    ctx: &AutomationCliContext,
    cmd: WakeCmd,
    gate: Option<Arc<Gate>>,
) -> Result<()> {
    let queue = WakeQueue::new(ctx.ledger.clone());
    match cmd {
        WakeCmd::List { json } => {
            let events = queue.events().await?;
            let pending = crate::wake::project_pending(&events);
            let unfinished = project_unfinished(&events);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "pending": pending.values().collect::<Vec<_>>(),
                        "unfinished": unfinished,
                    }))?
                );
            } else {
                for w in pending.values() {
                    println!(
                        "{} {} due {} — {}",
                        w.key,
                        w.wake_id,
                        w.due_at.to_rfc3339(),
                        w.message
                    );
                }
                for w in unfinished {
                    println!(
                        "UNFINISHED {} {} (claimed by a sweep that did not complete)",
                        w.key, w.wake_id
                    );
                }
            }
        }
        WakeCmd::Due { at, json } => {
            let due = queue.due(at_or_now(at.as_deref())?).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&due)?);
            } else {
                for w in due {
                    println!(
                        "{} {} due {} — {}",
                        w.key,
                        w.wake_id,
                        w.due_at.to_rfc3339(),
                        w.message
                    );
                }
            }
        }
        WakeCmd::RunDue { at, json } => {
            let config = AutomationConfig::load(&ctx.root)?;
            let handler: Option<&dyn NodeTimerHandler> =
                gate.as_deref().map(|g| g as &dyn NodeTimerHandler);
            let report = run_due(
                &SweepContext {
                    root: ctx.root.clone(),
                    ledger: ctx.ledger.clone(),
                    config,
                    node_handler: handler,
                },
                at_or_now(at.as_deref())?,
            )
            .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if report.locked {
                println!("locked: another sweep is running");
            } else {
                for r in &report.released {
                    println!("released attention {} -> {}", r.item_id, r.decision);
                }
                for f in &report.fired {
                    let first = f.detail.lines().next().unwrap_or_default();
                    println!("fired {} {} -> {} ({first})", f.key, f.wake_id, f.outcome);
                }
                for w in &report.unfinished {
                    println!("UNFINISHED {} {}", w.key, w.wake_id);
                }
                if report.fired.is_empty() {
                    println!("no wakes due");
                }
            }
        }
        WakeCmd::Cancel { key, reason } => {
            match queue
                .cancel(&key, reason.as_deref().unwrap_or("cancelled"))
                .await?
            {
                Some(w) => println!("cancelled {} ({})", key, w.wake_id),
                None => println!("no pending wake for {key}"),
            }
        }
    }
    Ok(())
}

pub async fn run_attention_command(ctx: &AutomationCliContext, cmd: AttentionCmd) -> Result<()> {
    let cfg = AutomationConfig::load(&ctx.root)?;
    let gate = AttentionGate::new(&ctx.root, ctx.ledger.clone(), &cfg.attention)?;
    match cmd {
        AttentionCmd::List { open, json } => {
            let items: Vec<_> = gate
                .items()
                .await?
                .into_iter()
                .filter(|i| !open || i.is_open())
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                for i in items {
                    let state = match &i.state {
                        ItemState::Queued { release_at, reason } => {
                            format!("queued ({reason}) until {release_at}")
                        }
                        ItemState::Coalesced { into } => format!("coalesced into {into}"),
                        ItemState::Delivered { delivered_at } => {
                            format!("open since {delivered_at}")
                        }
                        ItemState::Acked { acked_by, .. } => format!("acked by {acked_by}"),
                    };
                    println!(
                        "{} [{}] {} — {} ({state})",
                        i.item_id, i.channel, i.key, i.title
                    );
                }
            }
        }
        AttentionCmd::Submit {
            key,
            title,
            body,
            channel,
            source,
        } => {
            let out = gate
                .submit(
                    AttentionRequest {
                        key,
                        channel: match channel {
                            ChannelArg::NeedsYou => AttentionChannel::NeedsYou,
                            ChannelArg::Mail => AttentionChannel::Mail,
                        },
                        title,
                        body,
                        source,
                    },
                    Utc::now(),
                )
                .await?;
            println!("{}", serde_json::to_string(&out)?);
        }
        AttentionCmd::Release { at } => {
            for r in gate.release_due(at_or_now(at.as_deref())?).await? {
                println!("{} -> {}", r.item_id, r.decision);
            }
        }
        AttentionCmd::Ack { item_id, by } => {
            gate.ack(&item_id, &by, Utc::now()).await?;
            println!("acked {item_id}");
        }
    }
    Ok(())
}
