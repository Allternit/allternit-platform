//! Campaigns: a declared objective with an owner, a budget, an executor and
//! exactly one pending check (the Raven Oncall `ops_check_later` shape, on
//! CommRails' ledger instead of an in-memory scheduler).
//!
//! Truth is the ledger (`Campaign*` events plus the campaign's `Wake*`
//! events keyed `campaign:<id>`). [`project_campaigns`] rebuilds every
//! campaign from those events; `.allternit/rails/campaigns/<id>.json` is a
//! derived view rewritten after each mutation.
//!
//! See `spec/CAMPAIGNS.md`.

pub mod budget;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::attention::{AttentionChannel, AttentionConfig, AttentionGate, AttentionRequest};
use crate::core::ids::create_event_id;
use crate::core::io::{ensure_dir, write_json_atomic};
use crate::core::types::{Actor, ActorType, AllternitEvent, LedgerQuery};
use crate::ledger::Ledger;
use crate::wake::{self, Wake, WakeQueue, WakeTarget};

pub use budget::{total_spent, Budget, BudgetMode, SpendEntry};

/// Derived per-campaign views.
pub const CAMPAIGN_VIEW_DIR: &str = ".allternit/rails/campaigns";
/// Minimum check-later delay.
pub const CHECK_FLOOR_SECS: i64 = 60;
/// Default maximum check-later delay (7 days); configurable.
pub const DEFAULT_CHECK_CEILING_SECS: i64 = 7 * 24 * 3600;

pub const CAMPAIGN_EVENT_TYPES: &[&str] = &[
    "CampaignDeclared",
    "CampaignNoteAdded",
    "CampaignSpendRecorded",
    "CampaignStatusChanged",
    "CampaignBudgetChanged",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum CampaignStatus {
    #[default]
    Active,
    Paused,
    Finished,
    Killed,
}

impl CampaignStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, CampaignStatus::Finished | CampaignStatus::Killed)
    }
}

impl std::fmt::Display for CampaignStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CampaignStatus::Active => "active",
            CampaignStatus::Paused => "paused",
            CampaignStatus::Finished => "finished",
            CampaignStatus::Killed => "killed",
        };
        write!(f, "{s}")
    }
}

/// Who acts when a campaign check fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "target", rename_all = "snake_case")]
pub enum CampaignExecutor {
    /// `bot:<slug>` — a Grok/Allternit bot.
    Bot(String),
    /// `ao:<harness>` — an ao-engine harness session.
    Ao(String),
    /// A shell command (runs only when the operator enabled + allowlisted it).
    Command(String),
}

impl CampaignExecutor {
    /// Parse `bot:<slug>` / `ao:<harness>` / `command` (+ the command string).
    pub fn parse(executor: &str, command: Option<&str>) -> Result<Self> {
        if executor == "command" {
            let cmd = command
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .context("executor `command` needs a command string")?;
            return Ok(CampaignExecutor::Command(cmd.to_string()));
        }
        if command.is_some() {
            bail!("a command string is only valid with executor `command`");
        }
        crate::work::types::validate_executor(executor).map_err(|e| anyhow::anyhow!(e))?;
        let (kind, name) = executor.split_once(':').expect("validated");
        Ok(match kind {
            "bot" => CampaignExecutor::Bot(name.to_string()),
            _ => CampaignExecutor::Ao(name.to_string()),
        })
    }

    /// `bot` / `ao` / `command` — the name the operator enables in config.
    pub fn kind(&self) -> &'static str {
        match self {
            CampaignExecutor::Bot(_) => "bot",
            CampaignExecutor::Ao(_) => "ao",
            CampaignExecutor::Command(_) => "command",
        }
    }

    pub fn label(&self) -> String {
        match self {
            CampaignExecutor::Bot(s) => format!("bot:{s}"),
            CampaignExecutor::Ao(s) => format!("ao:{s}"),
            CampaignExecutor::Command(c) => format!("command: {c}"),
        }
    }
}

/// Automatic re-arm after a check fires: either a fixed interval or weekly
/// wall-clock slots in a timezone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rearm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_secs: Option<i64>,
    /// `mon`..`sun`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<String>,
    /// Local `HH:MM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl Rearm {
    pub fn validate(&self) -> Result<()> {
        match (self.every_secs, self.at.as_deref()) {
            (Some(s), None) if self.weekdays.is_empty() => {
                if s < CHECK_FLOOR_SECS {
                    bail!("rearm.every_secs must be >= {CHECK_FLOOR_SECS}");
                }
                Ok(())
            }
            (None, Some(at)) => {
                NaiveTime::parse_from_str(at, "%H:%M")
                    .with_context(|| format!("rearm.at {at:?} must be HH:MM"))?;
                if self.weekdays.is_empty() {
                    bail!("rearm.weekdays must list at least one day");
                }
                for d in &self.weekdays {
                    parse_weekday(d)?;
                }
                self.tz()?;
                Ok(())
            }
            _ => bail!("rearm needs either every_secs, or weekdays + at (+ timezone)"),
        }
    }

    fn tz(&self) -> Result<chrono_tz::Tz> {
        let name = self.timezone.as_deref().unwrap_or("UTC");
        name.parse()
            .map_err(|e| anyhow::anyhow!("rearm.timezone {name:?}: {e}"))
    }

    /// Next firing strictly after `now`.
    pub fn next_after(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
        self.validate()?;
        if let Some(s) = self.every_secs {
            return Ok(now + Duration::seconds(s));
        }
        let tz = self.tz()?;
        let at = NaiveTime::parse_from_str(self.at.as_deref().unwrap_or_default(), "%H:%M")?;
        let days: Vec<Weekday> = self
            .weekdays
            .iter()
            .map(|d| parse_weekday(d))
            .collect::<Result<_>>()?;
        let mut date = now.with_timezone(&tz).date_naive();
        for _ in 0..8 {
            if days.contains(&date.weekday()) {
                let t = crate::attention::policy::resolve_local(tz, date, at);
                if t > now {
                    return Ok(t);
                }
            }
            date = date.succ_opt().context("date overflow")?;
        }
        bail!("no rearm slot within a week")
    }
}

fn parse_weekday(s: &str) -> Result<Weekday> {
    s.parse::<Weekday>()
        .map_err(|_| anyhow::anyhow!("unknown weekday {s:?} (use mon..sun)"))
}

/// Declared budget (no `spent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetDecl {
    pub unit: String,
    pub limit: f64,
    #[serde(default)]
    pub mode: BudgetMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_wake: Option<f64>,
}

/// A campaign definition: CLI flags or a YAML/JSON file
/// (`docs/examples/campaigns/*.yaml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignDefinition {
    pub id: String,
    pub objective: String,
    pub owner: String,
    #[serde(default)]
    pub status: CampaignStatus,
    /// `bot:<slug>` | `ao:<harness>` | `command`.
    pub executor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetDecl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rearm: Option<Rearm>,
}

impl CampaignDefinition {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading campaign definition {}", path.display()))?;
        let def: CampaignDefinition = if path.extension().is_some_and(|e| e == "json") {
            serde_json::from_str(&text)?
        } else {
            serde_yaml::from_str(&text)?
        };
        Ok(def)
    }

    pub fn validate(&self) -> Result<CampaignExecutor> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            bail!("campaign id {:?} must be non-empty [A-Za-z0-9_.-]", self.id);
        }
        if self.objective.trim().is_empty() {
            bail!("campaign objective is required");
        }
        if self.owner.trim().is_empty() {
            bail!("campaign owner is required");
        }
        if self.status.is_terminal() {
            bail!("a campaign is declared active or paused");
        }
        if let Some(b) = &self.budget {
            if b.unit.trim().is_empty() {
                bail!("budget.unit is required (declared, not interpreted)");
            }
            if b.limit.is_nan() || b.limit <= 0.0 {
                bail!("budget.limit must be > 0");
            }
            if b.per_wake.is_some_and(|p| p < 0.0) {
                bail!("budget.per_wake must be >= 0");
            }
        }
        if let Some(r) = &self.rearm {
            r.validate()?;
        }
        CampaignExecutor::parse(&self.executor, self.command.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignNote {
    pub at: String,
    pub by: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingCheck {
    pub wake_id: String,
    pub due_at: DateTime<Utc>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Campaign {
    pub campaign_id: String,
    pub objective: String,
    pub owner: String,
    pub status: CampaignStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    pub executor: CampaignExecutor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rearm: Option<Rearm>,
    /// The one pending check (a wake keyed `campaign:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_check: Option<PendingCheck>,
    #[serde(default)]
    pub notes: Vec<CampaignNote>,
    #[serde(default)]
    pub spends: Vec<SpendEntry>,
    pub declared_at: String,
    pub updated_at: String,
}

fn event_types() -> Vec<String> {
    CAMPAIGN_EVENT_TYPES
        .iter()
        .chain(wake::WAKE_EVENT_TYPES.iter())
        .map(|s| s.to_string())
        .collect()
}

/// Rebuild every campaign from ledger events (campaign + wake events).
pub fn project_campaigns(events: &[AllternitEvent]) -> BTreeMap<String, Campaign> {
    let mut out: BTreeMap<String, Campaign> = BTreeMap::new();
    for evt in events {
        let p = &evt.payload;
        let Some(id) = p.get("campaign_id").and_then(|v| v.as_str()) else {
            continue;
        };
        match evt.r#type.as_str() {
            "CampaignDeclared" => {
                let Ok(def) = serde_json::from_value::<CampaignDefinition>(p["definition"].clone())
                else {
                    continue;
                };
                let Ok(executor) = CampaignExecutor::parse(&def.executor, def.command.as_deref())
                else {
                    continue;
                };
                out.entry(id.to_string()).or_insert(Campaign {
                    campaign_id: id.to_string(),
                    objective: def.objective,
                    owner: def.owner,
                    status: def.status,
                    status_reason: None,
                    executor,
                    budget: def.budget.map(|b| Budget {
                        unit: b.unit,
                        limit: b.limit,
                        mode: b.mode,
                        per_wake: b.per_wake,
                        spent: 0.0,
                    }),
                    dag_id: def.dag_id,
                    rearm: def.rearm,
                    pending_check: None,
                    notes: Vec::new(),
                    spends: Vec::new(),
                    declared_at: evt.ts.clone(),
                    updated_at: evt.ts.clone(),
                });
            }
            "CampaignNoteAdded" => {
                if let Some(c) = out.get_mut(id) {
                    c.notes.push(CampaignNote {
                        at: evt.ts.clone(),
                        by: p["by"].as_str().unwrap_or_default().to_string(),
                        text: p["text"].as_str().unwrap_or_default().to_string(),
                    });
                    c.updated_at = evt.ts.clone();
                }
            }
            "CampaignSpendRecorded" => {
                if let (Some(c), Ok(entry)) = (
                    out.get_mut(id),
                    serde_json::from_value::<SpendEntry>(p["entry"].clone()),
                ) {
                    c.spends.push(entry);
                    if let Some(b) = c.budget.as_mut() {
                        b.spent = total_spent(b.mode, &c.spends);
                    }
                    c.updated_at = evt.ts.clone();
                }
            }
            "CampaignStatusChanged" => {
                if let (Some(c), Ok(to)) = (
                    out.get_mut(id),
                    serde_json::from_value::<CampaignStatus>(p["to"].clone()),
                ) {
                    c.status = to;
                    c.status_reason = p["reason"].as_str().map(str::to_string);
                    c.updated_at = evt.ts.clone();
                }
            }
            "CampaignBudgetChanged" => {
                if let Some(c) = out.get_mut(id) {
                    if let (Some(b), Some(limit)) = (c.budget.as_mut(), p["limit"].as_f64()) {
                        b.limit = limit;
                    }
                    c.updated_at = evt.ts.clone();
                }
            }
            _ => {}
        }
    }
    let pending = wake::project_pending(events);
    for c in out.values_mut() {
        c.pending_check = pending
            .get(&wake::campaign_key(&c.campaign_id))
            .map(|w| PendingCheck {
                wake_id: w.wake_id.clone(),
                due_at: w.due_at,
                message: w.message.clone(),
            });
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckArmed {
    pub wake: Wake,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced: Option<Wake>,
    /// `floor` or `ceiling` when the requested delay was clamped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clamped: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpendOutcome {
    pub campaign: Campaign,
    /// True when this spend exhausted the budget and paused the campaign.
    pub paused_for_budget: bool,
}

pub struct CampaignOps {
    root: PathBuf,
    ledger: Arc<Ledger>,
    check_ceiling_secs: i64,
    attention: AttentionConfig,
    actor: Actor,
}

impl CampaignOps {
    pub fn new(
        root: impl Into<PathBuf>,
        ledger: Arc<Ledger>,
        check_ceiling_secs: i64,
        attention: AttentionConfig,
    ) -> Self {
        Self {
            root: root.into(),
            ledger,
            check_ceiling_secs: check_ceiling_secs.max(CHECK_FLOOR_SECS),
            attention,
            actor: Actor {
                r#type: ActorType::User,
                id: "cli".to_string(),
            },
        }
    }

    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    fn queue(&self) -> WakeQueue {
        WakeQueue::new(self.ledger.clone())
    }

    pub async fn all(&self) -> Result<BTreeMap<String, Campaign>> {
        let events = self
            .ledger
            .query(LedgerQuery {
                types: Some(event_types()),
                ..Default::default()
            })
            .await?;
        Ok(project_campaigns(&events))
    }

    pub async fn get(&self, id: &str) -> Result<Campaign> {
        self.all()
            .await?
            .remove(id)
            .with_context(|| format!("campaign {id} not declared"))
    }

    pub async fn declare(&self, def: CampaignDefinition, now: DateTime<Utc>) -> Result<Campaign> {
        def.validate()?;
        if self.all().await?.contains_key(&def.id) {
            bail!("campaign {} already declared", def.id);
        }
        let id = def.id.clone();
        let status = def.status;
        let rearm = def.rearm.clone();
        self.emit("CampaignDeclared", &id, json!({ "definition": def }))
            .await?;
        if status == CampaignStatus::Active {
            if let Some(r) = rearm {
                let due = r.next_after(now)?;
                self.arm(&id, due, "scheduled check (rearm)").await?;
            }
        }
        self.refresh_view(&id).await
    }

    pub async fn note(&self, id: &str, text: &str) -> Result<Campaign> {
        self.get(id).await?;
        self.emit(
            "CampaignNoteAdded",
            id,
            json!({ "text": text, "by": self.actor_label() }),
        )
        .await?;
        self.refresh_view(id).await
    }

    /// Record spend. When it exhausts the budget of an active campaign, the
    /// campaign is paused and a needs-you item raised through the attention gate.
    pub async fn spend(
        &self,
        id: &str,
        entry: SpendEntry,
        now: DateTime<Utc>,
    ) -> Result<SpendOutcome> {
        let c = self.get(id).await?;
        if c.status.is_terminal() {
            bail!("campaign {id} is {}; spend is closed", c.status);
        }
        if entry.amount.is_nan() || entry.amount < 0.0 {
            bail!("spend amount must be >= 0");
        }
        if entry.start.is_some() != entry.end.is_some() {
            bail!("a spend span needs both start and end");
        }
        if let (Some(s), Some(e)) = (entry.start, entry.end) {
            if e <= s {
                bail!("spend span end must be after start");
            }
        }
        self.emit("CampaignSpendRecorded", id, json!({ "entry": entry }))
            .await?;
        let after = self.get(id).await?;
        let mut paused_for_budget = false;
        if let Some(b) = &after.budget {
            if b.exhausted() && after.status == CampaignStatus::Active {
                self.change_status(&after, CampaignStatus::Paused, "budget_exhausted")
                    .await?;
                paused_for_budget = true;
                let gate = AttentionGate::new(&self.root, self.ledger.clone(), &self.attention)?;
                gate.submit(
                    AttentionRequest {
                        key: format!("campaign:{id}:budget"),
                        channel: AttentionChannel::NeedsYou,
                        title: format!("Campaign {id} paused: budget exhausted"),
                        body: format!(
                            "Campaign {id} ({}) spent {} of {} {} and was paused. \
                             Resume with a higher limit (`campaign resume {id} --limit <n>`) \
                             or finish/kill it. Budgets bound recorded spend only; they do not \
                             cap provider bills.",
                            after.objective, b.spent, b.limit, b.unit
                        ),
                        source: format!("campaign:{id}"),
                    },
                    now,
                )
                .await?;
            }
        }
        Ok(SpendOutcome {
            campaign: self.refresh_view(id).await?,
            paused_for_budget,
        })
    }

    pub async fn pause(&self, id: &str, reason: Option<&str>) -> Result<Campaign> {
        let c = self.get(id).await?;
        if c.status != CampaignStatus::Active {
            bail!(
                "campaign {id} is {}; only an active campaign can be paused",
                c.status
            );
        }
        self.change_status(&c, CampaignStatus::Paused, reason.unwrap_or("paused"))
            .await?;
        self.refresh_view(id).await
    }

    /// Resume a paused campaign. An exhausted budget must be raised
    /// (`new_limit` > spent) first. A campaign with `rearm` and no pending
    /// check is re-armed.
    pub async fn resume(
        &self,
        id: &str,
        new_limit: Option<f64>,
        now: DateTime<Utc>,
    ) -> Result<Campaign> {
        let c = self.get(id).await?;
        if c.status != CampaignStatus::Paused {
            bail!(
                "campaign {id} is {}; only a paused campaign can be resumed",
                c.status
            );
        }
        if let Some(limit) = new_limit {
            let Some(b) = &c.budget else {
                bail!("campaign {id} has no budget to raise");
            };
            if limit.is_nan() || limit <= 0.0 {
                bail!("--limit must be > 0");
            }
            self.emit(
                "CampaignBudgetChanged",
                id,
                json!({ "limit": limit, "previous_limit": b.limit }),
            )
            .await?;
        }
        let c = self.get(id).await?;
        if let Some(b) = &c.budget {
            if b.exhausted() {
                bail!(
                    "campaign {id} budget exhausted ({} of {} {}); resume with --limit above {}",
                    b.spent,
                    b.limit,
                    b.unit,
                    b.spent
                );
            }
        }
        self.change_status(&c, CampaignStatus::Active, "resumed")
            .await?;
        if c.pending_check.is_none() {
            if let Some(r) = &c.rearm {
                self.arm(id, r.next_after(now)?, "scheduled check (rearm)")
                    .await?;
            }
        }
        self.refresh_view(id).await
    }

    pub async fn kill(&self, id: &str, reason: Option<&str>) -> Result<Campaign> {
        self.conclude(id, CampaignStatus::Killed, reason.unwrap_or("killed"))
            .await
    }

    pub async fn finish(&self, id: &str, reason: Option<&str>) -> Result<Campaign> {
        self.conclude(id, CampaignStatus::Finished, reason.unwrap_or("finished"))
            .await
    }

    async fn conclude(&self, id: &str, to: CampaignStatus, reason: &str) -> Result<Campaign> {
        let c = self.get(id).await?;
        if c.status.is_terminal() {
            bail!("campaign {id} is already {}", c.status);
        }
        self.change_status(&c, to, reason).await?;
        // A concluded campaign must not come back: stand its check down.
        self.queue()
            .cancel(&wake::campaign_key(id), &format!("campaign {to}"))
            .await?;
        self.refresh_view(id).await
    }

    /// Arm (or re-arm, replacing) the campaign's one pending check.
    /// Delay is clamped to [60s, ceiling].
    pub async fn check_later(
        &self,
        id: &str,
        delay_secs: i64,
        message: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<CheckArmed> {
        let c = self.get(id).await?;
        if c.status.is_terminal() {
            bail!("campaign {id} is {}; it cannot be checked again", c.status);
        }
        let (delay, clamped) = clamp_delay(delay_secs, self.check_ceiling_secs);
        let (wake, replaced) = self
            .arm(
                id,
                now + Duration::seconds(delay),
                message.unwrap_or("check the campaign and decide the next step"),
            )
            .await?;
        self.refresh_view(id).await?;
        Ok(CheckArmed {
            wake,
            replaced,
            clamped,
        })
    }

    pub(crate) async fn arm(
        &self,
        id: &str,
        due: DateTime<Utc>,
        message: &str,
    ) -> Result<(Wake, Option<Wake>)> {
        self.queue()
            .schedule(
                &wake::campaign_key(id),
                WakeTarget::Campaign {
                    campaign_id: id.to_string(),
                },
                due,
                message,
                &self.actor_label(),
            )
            .await
    }

    async fn change_status(&self, c: &Campaign, to: CampaignStatus, reason: &str) -> Result<()> {
        self.emit(
            "CampaignStatusChanged",
            &c.campaign_id,
            json!({ "from": c.status, "to": to, "reason": reason }),
        )
        .await
    }

    fn actor_label(&self) -> String {
        let t = match self.actor.r#type {
            ActorType::User => "user",
            ActorType::Agent => "agent",
            ActorType::Gate => "gate",
        };
        format!("{t}:{}", self.actor.id)
    }

    async fn emit(&self, r#type: &str, id: &str, mut payload: serde_json::Value) -> Result<()> {
        payload["campaign_id"] = json!(id);
        self.ledger
            .append(AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: self.actor.clone(),
                scope: None,
                r#type: r#type.to_string(),
                payload,
                provenance: None,
            })
            .await?;
        Ok(())
    }

    /// Rewrite the derived view and return the projected campaign.
    pub async fn refresh_view(&self, id: &str) -> Result<Campaign> {
        let c = self.get(id).await?;
        let dir = self.root.join(CAMPAIGN_VIEW_DIR);
        ensure_dir(&dir)?;
        write_json_atomic(&dir.join(format!("{id}.json")), &c)?;
        Ok(c)
    }
}

/// Clamp a check-later delay to [floor, ceiling].
pub fn clamp_delay(delay_secs: i64, ceiling_secs: i64) -> (i64, Option<&'static str>) {
    if delay_secs < CHECK_FLOOR_SECS {
        (CHECK_FLOOR_SECS, Some("floor"))
    } else if delay_secs > ceiling_secs {
        (ceiling_secs, Some("ceiling"))
    } else {
        (delay_secs, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_parse() {
        assert_eq!(
            CampaignExecutor::parse("bot:chief", None).unwrap(),
            CampaignExecutor::Bot("chief".into())
        );
        assert_eq!(
            CampaignExecutor::parse("ao:claude", None).unwrap(),
            CampaignExecutor::Ao("claude".into())
        );
        assert_eq!(
            CampaignExecutor::parse("command", Some("echo hi")).unwrap(),
            CampaignExecutor::Command("echo hi".into())
        );
        assert!(CampaignExecutor::parse("command", None).is_err());
        assert!(CampaignExecutor::parse("bot:chief", Some("x")).is_err());
        assert!(CampaignExecutor::parse("shell:x", None).is_err());
    }

    #[test]
    fn clamp_floor_and_ceiling() {
        assert_eq!(clamp_delay(5, 3600), (60, Some("floor")));
        assert_eq!(clamp_delay(600, 3600), (600, None));
        assert_eq!(clamp_delay(99_999, 3600), (3600, Some("ceiling")));
    }

    #[test]
    fn weekday_rearm_across_dst() {
        let r = Rearm {
            every_secs: None,
            weekdays: ["mon", "tue", "wed", "thu", "fri"]
                .map(String::from)
                .to_vec(),
            at: Some("09:05".into()),
            timezone: Some("America/Chicago".into()),
        };
        // Fri 2026-03-06 10:00 CST -> Mon 2026-03-09 09:05 CDT (14:05Z).
        let now: DateTime<Utc> = "2026-03-06T16:00:00Z".parse().unwrap();
        assert_eq!(
            r.next_after(now).unwrap(),
            "2026-03-09T14:05:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        // Tue 2026-09-29 08:00 CDT -> same day 09:05 CDT.
        let now: DateTime<Utc> = "2026-09-29T13:00:00Z".parse().unwrap();
        assert_eq!(
            r.next_after(now).unwrap(),
            "2026-09-29T14:05:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn example_definitions_parse_and_stay_paused() {
        for (name, text) in [
            (
                "research-pipeline-sweep",
                include_str!("../../../docs/examples/campaigns/research-pipeline-sweep.yaml"),
            ),
            (
                "nightly-audit",
                include_str!("../../../docs/examples/campaigns/nightly-audit.yaml"),
            ),
        ] {
            let def: CampaignDefinition = serde_yaml::from_str(text).unwrap();
            def.validate().unwrap();
            assert_eq!(def.id, name);
            assert_eq!(
                def.status,
                CampaignStatus::Paused,
                "{name} must ship paused"
            );
        }
    }
}
