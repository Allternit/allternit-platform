//! Wake sweep: fire due wakes once, under the sweep lock.
//!
//! Order per sweep:
//! 1. take the sweep lock (a second concurrent sweep returns `locked`);
//! 2. release attention items whose quiet-hours/cap deferral has passed;
//! 3. re-read the ledger and fire each due wake: `WakeFired` (claim) first,
//!    then dispatch, then `WakeCompleted`. A sweep that dies between claim
//!    and completion leaves the wake in `unfinished` — it is not re-run
//!    (at-most-once), so a replay never repeats an executor run.
//!
//! Dispatch:
//! - `node_timer` → resolve elapsed timer gates on the node's DAG (the node
//!   becomes ready without polling); re-arm for any later timer on the node.
//! - `campaign` → only an **active** campaign is dispatched. A `command`
//!   executor runs only when the operator enabled `command` and allowlisted
//!   the exact string in `automation.yaml`; every other case (bot/ao, or a
//!   command not enabled) raises a needs-you item through the attention gate
//!   instead of spawning anything. `budget.per_wake` is recorded as spend,
//!   and `rearm` schedules the next check.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use crate::attention::{AttentionChannel, AttentionGate, AttentionRequest, SubmitOutcome};
use crate::campaign::{Campaign, CampaignExecutor, CampaignOps, CampaignStatus, SpendEntry};
use crate::core::types::LedgerQuery;
use crate::ledger::Ledger;
use crate::wait_gates::WaitGateKind;
use crate::wake::config::AutomationConfig;
use crate::wake::lock::SweepLock;
use crate::wake::{project_unfinished, Wake, WakeQueue, WakeTarget};
use crate::work::projection::project_dag;

/// Resolves elapsed timer wait-gates on a DAG (implemented by `Gate`).
#[async_trait]
pub trait NodeTimerHandler: Send + Sync {
    async fn resolve_elapsed_timers(&self, dag_id: &str) -> Result<Vec<String>>;
}

#[async_trait]
impl NodeTimerHandler for crate::gate::Gate {
    async fn resolve_elapsed_timers(&self, dag_id: &str) -> Result<Vec<String>> {
        self.resolve_elapsed_timer_gates(dag_id).await
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FiredWake {
    pub wake_id: String,
    pub key: String,
    pub outcome: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SweepReport {
    /// Another sweep holds the lock; nothing was fired.
    pub locked: bool,
    pub released: Vec<SubmitOutcome>,
    pub fired: Vec<FiredWake>,
    /// Claimed by an earlier sweep that never completed them.
    pub unfinished: Vec<Wake>,
}

pub struct SweepContext<'a> {
    pub root: PathBuf,
    pub ledger: Arc<Ledger>,
    pub config: AutomationConfig,
    /// Needed to fire node-timer wakes; without it they stay pending.
    pub node_handler: Option<&'a dyn NodeTimerHandler>,
}

/// Fire every wake due at `now`, once.
pub async fn run_due(ctx: &SweepContext<'_>, now: DateTime<Utc>) -> Result<SweepReport> {
    let Some(_lock) = SweepLock::try_acquire(&ctx.root)? else {
        return Ok(SweepReport {
            locked: true,
            ..Default::default()
        });
    };
    let attention = AttentionGate::new(&ctx.root, ctx.ledger.clone(), &ctx.config.attention)?;
    let mut report = SweepReport {
        released: attention.release_due(now).await?,
        ..Default::default()
    };
    let queue = WakeQueue::new(ctx.ledger.clone());
    let campaigns = CampaignOps::new(
        &ctx.root,
        ctx.ledger.clone(),
        ctx.config.campaign.check_ceiling_secs,
        ctx.config.attention.clone(),
    );

    // Read under the lock: anything an earlier sweep fired is gone.
    for wake in queue.due(now).await? {
        if matches!(wake.target, WakeTarget::NodeTimer { .. }) && ctx.node_handler.is_none() {
            continue;
        }
        queue
            .append(
                "WakeFired",
                json!({ "wake_id": wake.wake_id, "key": wake.key, "fired_at": now.to_rfc3339() }),
            )
            .await?;
        let (outcome, detail) = match &wake.target {
            WakeTarget::NodeTimer { dag_id, node_id } => {
                fire_node_timer(ctx, &queue, dag_id, node_id, now).await?
            }
            WakeTarget::Campaign { campaign_id } => {
                fire_campaign(ctx, &campaigns, &attention, campaign_id, &wake, now).await?
            }
        };
        queue
            .append(
                "WakeCompleted",
                json!({ "wake_id": wake.wake_id, "key": wake.key, "outcome": outcome, "detail": detail }),
            )
            .await?;
        report.fired.push(FiredWake {
            wake_id: wake.wake_id.clone(),
            key: wake.key.clone(),
            outcome,
            detail,
        });
    }
    report.unfinished = project_unfinished(&queue.events().await?);
    Ok(report)
}

async fn fire_node_timer(
    ctx: &SweepContext<'_>,
    queue: &WakeQueue,
    dag_id: &str,
    node_id: &str,
    now: DateTime<Utc>,
) -> Result<(String, String)> {
    let handler = ctx.node_handler.expect("checked by caller");
    let resolved = handler.resolve_elapsed_timers(dag_id).await?;
    // Re-arm for any timer on this node that is still in the future.
    let events: Vec<_> = ctx
        .ledger
        .query(LedgerQuery::default())
        .await?
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .collect();
    let dag = project_dag(&events, dag_id);
    let next = dag.nodes.get(node_id).and_then(|n| {
        n.wait_gates
            .iter()
            .filter(|g| g.kind == WaitGateKind::Timer && g.outcome.is_none())
            .filter_map(|g| {
                g.params
                    .get("until")?
                    .as_str()?
                    .parse::<DateTime<Utc>>()
                    .ok()
            })
            .filter(|u| *u > now)
            .min()
    });
    let mut detail = format!(
        "resolved {} timer gate(s): {}",
        resolved.len(),
        resolved.join(", ")
    );
    if let Some(next) = next {
        queue
            .schedule(
                &super::node_key(dag_id, node_id),
                WakeTarget::NodeTimer {
                    dag_id: dag_id.to_string(),
                    node_id: node_id.to_string(),
                },
                next,
                &format!("next timer wait-gate on {node_id}"),
                "wake:rearm",
            )
            .await?;
        detail.push_str(&format!("; re-armed for {}", next.to_rfc3339()));
    }
    Ok(("resolved_timers".to_string(), detail))
}

async fn fire_campaign(
    ctx: &SweepContext<'_>,
    ops: &CampaignOps,
    attention: &AttentionGate,
    campaign_id: &str,
    wake: &Wake,
    now: DateTime<Utc>,
) -> Result<(String, String)> {
    let Ok(c) = ops.get(campaign_id).await else {
        return Ok((
            "skipped".into(),
            format!("campaign {campaign_id} not declared"),
        ));
    };
    if c.status != CampaignStatus::Active {
        return Ok((
            format!("skipped_{}", c.status),
            format!("campaign is {}", c.status),
        ));
    }

    let (outcome, detail) = match &c.executor {
        CampaignExecutor::Command(cmd) if ctx.config.wake.command_runnable(cmd) => {
            let run = run_command(
                &ctx.root,
                &c,
                wake,
                cmd,
                ctx.config.wake.command_timeout_secs,
            )
            .await;
            match run {
                Ok((0, tail)) => ("ran".to_string(), format!("exit 0\n{tail}")),
                Ok((code, tail)) => {
                    raise(
                        attention,
                        &c,
                        "failed",
                        format!("Campaign {} check command failed", c.campaign_id),
                        format!("`{cmd}` exited {code}.\n\n{tail}"),
                        now,
                    )
                    .await?;
                    ("failed".to_string(), format!("exit {code}\n{tail}"))
                }
                Err(e) => {
                    raise(
                        attention,
                        &c,
                        "failed",
                        format!("Campaign {} check command did not run", c.campaign_id),
                        format!("`{cmd}`: {e}"),
                        now,
                    )
                    .await?;
                    ("failed".to_string(), e.to_string())
                }
            }
        }
        other => {
            let why = match other {
                CampaignExecutor::Command(_) => {
                    "its `command` executor is not enabled + allowlisted in .allternit/rails/automation.yaml".to_string()
                }
                e => format!(
                    "executor kind `{}` is never spawned by a sweep yet (spawn gating lands with the drive runner)",
                    e.kind()
                ),
            };
            let item = raise(
                attention,
                &c,
                "check",
                format!("Campaign {} check due", c.campaign_id),
                format!(
                    "Objective: {}\nExecutor: {}\nCheck: {}\n\nThis sweep did not run it: {why}. \
                     Run it yourself, then re-arm with `campaign check-later {} <secs>` or finish it.",
                    c.objective,
                    c.executor.label(),
                    wake.message,
                    c.campaign_id
                ),
                now,
            )
            .await?;
            (
                "needs_you".to_string(),
                format!("attention {} ({})", item.item_id, item.decision),
            )
        }
    };

    if let Some(per_wake) = c.budget.as_ref().and_then(|b| b.per_wake) {
        ops.spend(
            campaign_id,
            SpendEntry {
                amount: per_wake,
                start: None,
                end: None,
                resource: None,
                note: Some(format!("wake {}", wake.wake_id)),
            },
            now,
        )
        .await?;
    }
    let after = ops.get(campaign_id).await?;
    if after.status == CampaignStatus::Active && after.pending_check.is_none() {
        if let Some(r) = &after.rearm {
            ops.arm(campaign_id, r.next_after(now)?, "scheduled check (rearm)")
                .await?;
        }
    }
    ops.refresh_view(campaign_id).await?;
    Ok((outcome, detail))
}

async fn raise(
    attention: &AttentionGate,
    c: &Campaign,
    what: &str,
    title: String,
    body: String,
    now: DateTime<Utc>,
) -> Result<SubmitOutcome> {
    attention
        .submit(
            AttentionRequest {
                key: format!("campaign:{}:{what}", c.campaign_id),
                channel: AttentionChannel::NeedsYou,
                title,
                body,
                source: format!("campaign:{}", c.campaign_id),
            },
            now,
        )
        .await
}

const OUTPUT_TAIL: usize = 2000;

fn tail(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let start = s.len().saturating_sub(OUTPUT_TAIL);
    let start = (start..=s.len())
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(s.len());
    s[start..].to_string()
}

async fn run_command(
    root: &Path,
    c: &Campaign,
    wake: &Wake,
    cmd: &str,
    timeout_secs: u64,
) -> Result<(i32, String)> {
    #[cfg(unix)]
    let mut command = {
        let mut k = tokio::process::Command::new("/bin/sh");
        k.arg("-c").arg(cmd);
        k
    };
    #[cfg(not(unix))]
    let mut command = {
        let mut k = tokio::process::Command::new("cmd");
        k.arg("/C").arg(cmd);
        k
    };
    command
        .current_dir(root)
        .env("ALLTERNIT_CAMPAIGN_ID", &c.campaign_id)
        .env("ALLTERNIT_WAKE_ID", &wake.wake_id)
        .env("ALLTERNIT_WAKE_MESSAGE", &wake.message)
        .env("ALLTERNIT_RAILS_ROOT", root)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(dag) = &c.dag_id {
        command.env("ALLTERNIT_DAG_ID", dag);
    }
    let child = command.output();
    let out = tokio::time::timeout(StdDuration::from_secs(timeout_secs.max(1)), child)
        .await
        .map_err(|_| anyhow::anyhow!("timed out after {timeout_secs}s"))??;
    let mut combined = out.stdout.clone();
    combined.extend_from_slice(&out.stderr);
    Ok((out.status.code().unwrap_or(-1), tail(&combined)))
}
