//! `allternit-factory workflows drive <dag_id>`: the opt-in, foreground runner that
//! walks a WIH DAG and spawns the harness named by each READY node's
//! `executor` (spec/DRIVE.md).
//!
//! Drive is an explicit operator command, never a daemon: it exits when
//! nothing is READY or running, on Ctrl-C, or after one pass with `--once`.
//! Every step goes through the existing gates — Gate 1 pickup, open-sign,
//! the orchestrator spawn path with `--wih` (so the spawn gate's `admit()`
//! and PreToolUse hook apply), and Gate 4 `wih close` with the collected
//! output. Drive adds only scheduling, caps, and the attempt record.
//!
//! Stop rules (no silent retry, no retry loop):
//! * a node the runner cannot or must not advance gets a **manual wait-gate**
//!   (source `drive`), which puts it in the "needs you" list and keeps it out
//!   of `ready_nodes` until a human resolves it with `--actor`;
//! * an attempt interrupted after its harness started (the non-idempotent
//!   step) is restarted only for nodes labelled `retry:safe`, otherwise only
//!   after a human resolves the gate for that attempt;
//! * a dead or timed-out session closes the node FAILED with a receipt;
//! * a node with an `on_fail` route that closes failed is routed back to its
//!   target at most `max_rounds` times, then stops as degraded with a
//!   needs-you gate (see [`route`]);
//! * a `bot:<slug>` node whose bot is a **vendor** bot (given through
//!   [`DriveOptions::vendor_bots`], from `--team`) is picked up through Gate 1
//!   for agent `bot:<slug>` and delivered as an allternit-api vendor ticket
//!   (`DriveVendorTicketCreated`). Its open WIH then reads "waiting on vendor
//!   ticket T-n": never respawned, never an interrupted attempt. A ticket the
//!   API refuses hands the node to a person with the API's fact; there is no
//!   mail fallback. Other `bot:` nodes are mailed as before.

pub mod caps;
pub mod config;
pub mod hooks;
pub mod route;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::core::ids::create_event_id;
use crate::core::io::ensure_dir;
use crate::core::types::{Actor, ActorType, AllternitEvent, LedgerQuery};
use crate::gate::{Gate, GateError, WihPickupOptions};
use crate::hook::{self, WihPolicy};
use crate::ledger::Ledger;
use crate::mail::{Mail, MailImportance, MailOptions, TypedMessage};
use crate::agents::team_apply::{vendor_ticket_for_node, FactoryApi};
use crate::spawn::{self, session_alive_blocking, CaptureFiles, SpawnOptions, Spawner, WatchOutcome};
use crate::gate::gate::DagMutation;
use crate::templates::{CLOSURE_STATE, EVIDENCE_PARAM, RETRY_SAFE_LABEL};
use crate::wait_gates::WaitGateKind;
use crate::work::graph::ready_nodes;
use crate::work::projection::project_dag;
use crate::work::types::{DagNode, DagState};

use caps::{CapCounts, CapLimits, CapsStore, Capacity, Deferral, FileLock, RunningEntry};
use config::{drive_dir, ArgvVars, DriveConfig};
use hooks::{AttemptRef, DriveHooks, NeedsYou, NodeFinished};
pub use route::{FailRoute, ROUNDS_EXHAUSTED, ROUTE_BACK};

pub const ATTEMPT_STARTED: &str = "DriveAttemptStarted";
pub const ATTEMPT_FINISHED: &str = "DriveAttemptFinished";
pub const SPAWN_DEFERRED: &str = "DriveSpawnDeferred";
pub const NEEDS_YOU: &str = "DriveNeedsYou";
pub const BOT_NOTIFIED: &str = "DriveBotNotified";
pub const CAPACITY_REFUSED: &str = "DriveCapacityRefused";
pub const VENDOR_TICKET_CREATED: &str = "DriveVendorTicketCreated";

/// Needs-you reason when a vendor ticket could not be created.
pub const VENDOR_TICKET_FAILED: &str = "vendor_ticket_failed";

/// A vendor bot drive may hand nodes to (from the team's `team.yaml`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VendorInfo {
    pub vendor: Option<String>,
    pub lane: Option<String>,
    /// Team the bot belongs to (for the delivery's `to` address).
    pub team: Option<String>,
}

/// A vendor ticket drive created for a node's WIH (from the ledger).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorTicketRecord {
    pub node_id: String,
    pub wih_id: String,
    pub ticket: String,
    pub executor: String,
    pub lane: Option<String>,
    pub guarantee: Option<String>,
    pub at: String,
}

/// API.md `Delivery.state` for a ticket's guarantee.
pub fn ticket_delivery_state(guarantee: Option<&str>) -> &'static str {
    match guarantee {
        Some("exact") => "verified",
        Some("best_effort") => "best_effort",
        Some("read_only") => "read_only",
        _ => "failed",
    }
}

/// Vendor tickets of one DAG, keyed by `(node_id, wih_id)`, from
/// `DriveVendorTicketCreated` events.
pub fn project_vendor_tickets(events: &[AllternitEvent], dag_id: &str) -> BTreeMap<(String, String), VendorTicketRecord> {
    let mut out = BTreeMap::new();
    for e in events.iter().filter(|e| e.r#type == VENDOR_TICKET_CREATED) {
        let p = &e.payload;
        if p.get("dag_id").and_then(Value::as_str) != Some(dag_id) {
            continue;
        }
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(node_id), Some(wih_id), Some(ticket)) = (s("node_id"), s("wih_id"), s("ticket")) else { continue };
        out.insert(
            (node_id.clone(), wih_id.clone()),
            VendorTicketRecord {
                node_id,
                wih_id,
                ticket,
                executor: s("executor").unwrap_or_default(),
                lane: s("lane"),
                guarantee: s("guarantee"),
                at: e.ts.clone(),
            },
        );
    }
    out
}

/// Wait-gate param marking a gate drive created.
const GATE_SOURCE: &str = "drive";

/// Options for one `drive` invocation.
#[derive(Debug, Clone, Default)]
pub struct DriveOptions {
    pub dag_id: String,
    /// Per-DAG concurrent sessions for this run (≤ config `max_concurrent`).
    pub max_concurrent: Option<usize>,
    /// Per-DAG spawns per rolling hour for this run (≤ config).
    pub max_spawns_per_hour: Option<usize>,
    pub once: bool,
    pub dry_run: bool,
    /// Directory harnesses run in. Defaults to the workspace root.
    pub workdir: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
    /// Vendor bots by slug (from `--team`). A `bot:<slug>` node whose slug is
    /// here is delivered as a vendor ticket (needs [`Driver::with_vendor_api`]);
    /// empty keeps the mail notification for every `bot:` node.
    pub vendor_bots: BTreeMap<String, VendorInfo>,
}

/// Why the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveExit {
    /// Nothing READY and nothing running.
    Idle,
    /// `--once` pass complete (sessions may still be running; the next run adopts them).
    Once,
    /// Ctrl-C (sessions keep running; the next run adopts them).
    Interrupted,
    /// `--dry-run` plan printed.
    DryRun,
}

/// What a run did, for callers and tests. Every entry is also printed.
#[derive(Debug, Default)]
pub struct DriveReport {
    pub exit: Option<DriveExit>,
    /// `(node_id, attempt_id)` spawned.
    pub spawned: Vec<(String, String)>,
    /// `(node_id, outcome)` finished.
    pub finished: Vec<(String, String)>,
    /// `(node_id, reason)` handed to a human.
    pub needs_you: Vec<(String, String)>,
    /// `(node_id, deferral)`.
    pub deferred: Vec<(String, String)>,
    /// Nodes notified to their bot.
    pub bot_notified: Vec<String>,
    /// Final "waiting on" lines.
    pub waiting: Vec<String>,
    /// Dry-run plan lines.
    pub plan: Vec<String>,
    /// `(node_id, ticket, delivery state)` vendor tickets created this run.
    pub vendor_tickets: Vec<(String, String, String)>,
    /// `(failed node_id, on_fail target node_id, round)` routed back.
    pub routed_back: Vec<(String, String, u32)>,
    /// Failed nodes whose route-back rounds ran out (root closure degraded).
    pub degraded: Vec<String>,
}

/// One ledger attempt: a `DriveAttemptStarted` and its `DriveAttemptFinished`.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub attempt_id: String,
    pub node_id: String,
    pub wih_id: String,
    pub executor: String,
    pub slug: String,
    pub run_dir: PathBuf,
    pub started_at: DateTime<Utc>,
    pub timeout_seconds: u64,
    /// `None` while open.
    pub outcome: Option<String>,
}

impl Attempt {
    fn exit_file(&self) -> PathBuf {
        self.run_dir.join("exit_code")
    }

    fn as_ref(&self, dag_id: &str) -> AttemptRef {
        AttemptRef {
            dag_id: dag_id.to_string(),
            node_id: self.node_id.clone(),
            wih_id: self.wih_id.clone(),
            attempt_id: self.attempt_id.clone(),
            executor: self.executor.clone(),
            slug: self.slug.clone(),
        }
    }
}

/// Attempts of one DAG, in ledger order.
pub fn project_attempts(events: &[AllternitEvent], dag_id: &str) -> Vec<Attempt> {
    let mut out: Vec<Attempt> = Vec::new();
    for evt in events {
        let p = &evt.payload;
        if p.get("dag_id").and_then(Value::as_str) != Some(dag_id) {
            continue;
        }
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        match evt.r#type.as_str() {
            ATTEMPT_STARTED => out.push(Attempt {
                attempt_id: s("attempt_id"),
                node_id: s("node_id"),
                wih_id: s("wih_id"),
                executor: s("executor"),
                slug: s("slug"),
                run_dir: PathBuf::from(s("run_dir")),
                started_at: DateTime::parse_from_rfc3339(&evt.ts)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                timeout_seconds: p.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(3600),
                outcome: None,
            }),
            ATTEMPT_FINISHED => {
                let id = s("attempt_id");
                if let Some(a) = out.iter_mut().find(|a| a.attempt_id == id) {
                    a.outcome = Some(s("outcome"));
                }
            }
            _ => {}
        }
    }
    out
}

/// What the scheduler decided for one node this pass.
#[derive(Debug, Clone)]
enum Action {
    /// Pick up (or, with `wih`, restart on the existing WIH) and spawn.
    Spawn {
        node_id: String,
        harness: String,
        wih: Option<String>,
        restart_of: Option<String>,
    },
    /// Retry `wih close` from a finished attempt's captured output (after a
    /// human resolved the `close_failed` gate) — the harness is not re-run.
    Collect { attempt: Attempt },
    NotifyBot { node_id: String, slug: String },
    /// Deliver to a vendor bot: pick up (unless `wih` is the one already
    /// held for it), then create the ticket.
    VendorTicket { node_id: String, slug: String, wih: Option<String> },
    NeedsYou {
        node_id: String,
        reason: &'static str,
        detail: String,
        executor: String,
        attempt_id: Option<String>,
    },
    Wait { line: String },
}

struct Running {
    attempt: Attempt,
    deadline: DateTime<Utc>,
}

pub struct Driver {
    root: PathBuf,
    ledger: Arc<Ledger>,
    gate: Option<Arc<Gate>>,
    cfg: DriveConfig,
    opts: DriveOptions,
    hooks: Arc<dyn DriveHooks>,
    vendor_api: Option<Arc<dyn FactoryApi>>,
    caps: CapsStore,
    orch: Option<Spawner>,
    running: BTreeMap<String, Running>,
    /// `(node_id, reason)` deferrals already written to the ledger this run.
    deferral_logged: HashSet<(String, &'static str)>,
    last_status: Vec<String>,
}

impl Driver {
    /// `gate` may be `None` only for a dry run.
    pub fn new(
        root: PathBuf,
        ledger: Arc<Ledger>,
        gate: Option<Arc<Gate>>,
        opts: DriveOptions,
        hooks: Arc<dyn DriveHooks>,
    ) -> Result<Self> {
        if gate.is_none() && !opts.dry_run {
            bail!("drive needs a Gate unless --dry-run");
        }
        let cfg = DriveConfig::load(&root)?;
        let orch = if opts.dry_run {
            None
        } else {
            Some(Spawner::new(root.clone())?)
        };
        Ok(Self {
            caps: CapsStore::new(&root),
            root,
            ledger,
            gate,
            cfg,
            opts,
            hooks,
            vendor_api: None,
            orch,
            running: BTreeMap::new(),
            deferral_logged: HashSet::new(),
            last_status: Vec::new(),
        })
    }

    /// The allternit-api client vendor tickets are created through.
    pub fn with_vendor_api(mut self, api: Arc<dyn FactoryApi>) -> Self {
        self.vendor_api = Some(api);
        self
    }

    fn dag_id(&self) -> &str {
        &self.opts.dag_id
    }

    fn limits(&self) -> CapLimits {
        CapLimits {
            dag_concurrent: self
                .opts
                .max_concurrent
                .map_or(self.cfg.max_concurrent, |n| n.min(self.cfg.max_concurrent)),
            dag_per_hour: self
                .opts
                .max_spawns_per_hour
                .map_or(self.cfg.max_spawns_per_hour, |n| n.min(self.cfg.max_spawns_per_hour)),
            global_concurrent: self.cfg.global_max_concurrent,
            global_per_hour: self.cfg.global_max_spawns_per_hour,
        }
    }

    fn timeout_seconds(&self) -> u64 {
        self.opts.timeout_seconds.unwrap_or(self.cfg.timeout_seconds)
    }

    fn gate(&self) -> Result<&Arc<Gate>> {
        self.gate.as_ref().ok_or_else(|| anyhow!("no gate (dry run)"))
    }

    fn orch(&self) -> Result<&Spawner> {
        self.orch.as_ref().ok_or_else(|| anyhow!("no spawner (dry run)"))
    }

    async fn tickets(&self) -> Result<BTreeMap<(String, String), VendorTicketRecord>> {
        let events = self
            .ledger
            .query(LedgerQuery { r#type: Some(VENDOR_TICKET_CREATED.to_string()), ..Default::default() })
            .await?;
        Ok(project_vendor_tickets(&events, self.dag_id()))
    }

    async fn load(&self) -> Result<(Vec<AllternitEvent>, DagState, Vec<Attempt>)> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let dag_events: Vec<AllternitEvent> = events
            .iter()
            .filter(|e| e.payload.get("dag_id").and_then(Value::as_str) == Some(self.dag_id()))
            .cloned()
            .collect();
        let dag = project_dag(&dag_events, self.dag_id());
        let attempts = project_attempts(&dag_events, self.dag_id());
        Ok((events, dag, attempts))
    }

    /// Run until idle / `--once` / `stop` resolves (Ctrl-C in the CLI).
    pub async fn run(
        &mut self,
        stop: impl std::future::Future<Output = ()>,
    ) -> Result<DriveReport> {
        let mut report = DriveReport::default();
        let (_, dag, _) = self.load().await?;
        if dag.nodes.is_empty() {
            bail!("dag {} not found (no nodes in the ledger)", self.dag_id());
        }
        if self.opts.dry_run {
            self.plan(&mut report).await?;
            report.exit = Some(DriveExit::DryRun);
            return Ok(report);
        }

        let lock_path = drive_dir(&self.root)
            .join("dags")
            .join(format!("{}.lock", self.dag_id()));
        // A short patience window: a forked child (tmux, vm_stat) can hold an
        // inherited copy of the fd between fork and exec for a moment.
        let _dag_lock = FileLock::try_exclusive_for(&lock_path, std::time::Duration::from_secs(2))
            .await?
            .ok_or_else(|| {
            anyhow!(
                "another drive process is already driving {} (lock {})",
                self.dag_id(),
                lock_path.display()
            )
        })?;

        if let Err(reason) = Capacity::probe().admit(self.cfg.min_free_mem_mb, self.cfg.max_load_per_cpu) {
            self.emit(CAPACITY_REFUSED, json!({ "dag_id": self.dag_id(), "reason": reason }))
                .await?;
            bail!("capacity admission refused: {reason}");
        }

        tokio::pin!(stop);
        let poll = std::time::Duration::from_millis(self.cfg.poll_interval_ms.max(50));
        loop {
            let busy = self.tick(&mut report).await?;
            if self.opts.once {
                report.exit = Some(DriveExit::Once);
                break;
            }
            if !busy {
                report.exit = Some(DriveExit::Idle);
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(poll) => {}
                _ = &mut stop => {
                    report.exit = Some(DriveExit::Interrupted);
                    break;
                }
            }
        }
        report.waiting = self.last_status.clone();
        if !report.waiting.is_empty() {
            println!("drive {}: waiting on:", self.dag_id());
            for line in &report.waiting {
                println!("  {line}");
            }
        }
        if !self.running.is_empty() {
            println!(
                "drive {}: {} session(s) still running; the next `drive {}` adopts them:",
                self.dag_id(),
                self.running.len(),
                self.dag_id()
            );
            for r in self.running.values() {
                println!("  {} {}", r.attempt.node_id, spawn::session_name(&r.attempt.slug));
            }
        }
        Ok(report)
    }

    /// One scheduling pass. Returns true while there is something to wait
    /// for (own sessions running, or spawns deferred by caps/capacity).
    async fn tick(&mut self, report: &mut DriveReport) -> Result<bool> {
        let dag_id = self.dag_id().to_string();
        self.gate()?.resolve_elapsed_timer_gates(&dag_id).await?;

        // 1. Poll own sessions.
        let slugs: Vec<String> = self.running.keys().cloned().collect();
        for node_id in slugs {
            let (slug, exit_file, deadline, aref) = {
                let r = &self.running[&node_id];
                (r.attempt.slug.clone(), r.attempt.exit_file(), r.deadline, r.attempt.as_ref(&dag_id))
            };
            let outcome = match self.orch()?.poll(&slug, &exit_file).await {
                Some(o) => Some(o),
                None if Utc::now() >= deadline => Some(WatchOutcome::Timeout),
                None => {
                    if let Err(e) = self.hooks.on_attempt_heartbeat(&aref).await {
                        eprintln!("drive: heartbeat hook failed for {node_id}: {e:#}");
                    }
                    None
                }
            };
            if let Some(outcome) = outcome {
                let running = self.running.remove(&node_id).expect("running");
                self.finish(running.attempt, outcome, report).await?;
            }
        }

        // 2. Reconcile open ledger attempts this process does not hold:
        //    adopt live sessions, collect finished ones, mark the rest interrupted.
        let (_, dag, attempts) = self.load().await?;
        for a in attempts.iter().filter(|a| a.outcome.is_none()) {
            if self.running.contains_key(&a.node_id) {
                continue;
            }
            let node_open = dag
                .nodes
                .get(&a.node_id)
                .is_some_and(|n| n.current_wih_id.as_deref() == Some(a.wih_id.as_str()));
            if !node_open {
                // The WIH was closed after the harness ran (drive stopped
                // between close and its finish record, or someone closed it).
                self.record_finish(a, "closed", None, None, Some("WIH closed outside this attempt"))
                    .await?;
                continue;
            }
            if a.exit_file().exists() {
                println!("drive: collecting finished attempt {} ({})", a.attempt_id, a.node_id);
                self.finish(a.clone(), WatchOutcome::Done, report).await?;
            } else if spawn::session_alive(&a.slug).await {
                println!("drive: adopting running session {} ({})", spawn::session_name(&a.slug), a.node_id);
                self.caps.adopt(self.running_entry(&a.node_id, &a.slug), &session_alive_blocking)?;
                let deadline = a.started_at + chrono::Duration::seconds(a.timeout_seconds as i64);
                self.running.insert(a.node_id.clone(), Running { attempt: a.clone(), deadline });
            } else {
                println!(
                    "drive: attempt {} on {} was interrupted (session gone, no exit code)",
                    a.attempt_id, a.node_id
                );
                self.record_finish(a, "interrupted", None, None, Some("session gone without an exit code"))
                    .await?;
            }
        }

        // 2b. Failed nodes with an `on_fail` route: route back (bounded) or
        //     stop as degraded.
        let route_lines = self.route_failures(report).await?;

        // 3. Decide.
        let (_, dag, attempts) = self.load().await?;
        let mut live = HashSet::new();
        for r in self.running.values() {
            live.insert(r.attempt.attempt_id.clone());
        }
        let tickets = self.tickets().await?;
        let actions = self.decide(&dag, &attempts, &live, &tickets);

        // 4. Act.
        let mut deferred_any = false;
        let mut status = route_lines;
        for action in actions {
            match action {
                Action::Wait { line } => status.push(line),
                Action::Collect { attempt } => {
                    println!("drive: retrying wih close for {} from attempt {}", attempt.node_id, attempt.attempt_id);
                    self.finish(attempt, WatchOutcome::Done, report).await?;
                }
                Action::VendorTicket { node_id, slug, wih } => {
                    status.push(self.vendor_ticket(&dag, &node_id, &slug, wih, report).await?);
                }
                Action::NotifyBot { node_id, slug } => {
                    self.notify_bot(&dag, &node_id, &slug).await?;
                    report.bot_notified.push(node_id.clone());
                    status.push(format!("{node_id}: executor bot:{slug} (notified by mail; drive does not spawn bots)"));
                }
                Action::NeedsYou { node_id, reason, detail, executor, attempt_id } => {
                    let gate_id = self
                        .needs_you(&node_id, reason, &detail, &executor, attempt_id.as_deref())
                        .await?;
                    report.needs_you.push((node_id.clone(), reason.to_string()));
                    status.push(format!("{node_id}: needs you [{gate_id}] {detail}"));
                }
                Action::Spawn { node_id, harness, wih, restart_of } => {
                    match self.start(&dag, &node_id, &harness, wih, restart_of, report).await? {
                        Started::Spawned => {}
                        Started::Deferred(d) => {
                            deferred_any = true;
                            status.push(format!("{node_id}: spawn deferred: {d}"));
                        }
                        Started::NeedsYou(line) => status.push(line),
                    }
                }
            }
        }
        for r in self.running.values() {
            status.push(format!(
                "{}: running {} ({})",
                r.attempt.node_id,
                spawn::session_name(&r.attempt.slug),
                r.attempt.executor
            ));
        }
        status.sort();
        if status != self.last_status {
            for line in &status {
                println!("drive {}: {line}", self.dag_id());
            }
            self.last_status = status;
        }
        Ok(!self.running.is_empty() || deferred_any)
    }

    /// Pure scheduling decision for every non-terminal node.
    fn decide(
        &self,
        dag: &DagState,
        attempts: &[Attempt],
        live: &HashSet<String>,
        tickets: &BTreeMap<(String, String), VendorTicketRecord>,
    ) -> Vec<Action> {
        let ready: HashSet<String> = ready_nodes(dag).into_iter().collect();
        let now = Utc::now();
        let mut nodes: Vec<&DagNode> = dag.nodes.values().collect();
        nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        let mut out = Vec::new();
        for node in nodes {
            if node.status == "DONE" || node.status == "FAILED" {
                continue;
            }
            let latest = attempts
                .iter()
                .rev()
                .find(|a| a.node_id == node.node_id && Some(a.wih_id.as_str()) == node.current_wih_id.as_deref());
            if let Some(a) = latest {
                if live.contains(&a.attempt_id) {
                    continue; // reported as running
                }
            }
            // A vendor bot's open WIH: waiting on its ticket, or (picked up
            // for this bot, no ticket yet) create it, unless a person still
            // has to look at the last refusal.
            if let (Some(slug), Some(wih)) = (self.vendor_slug(node), node.current_wih_id.as_deref()) {
                if let Some(t) = tickets.get(&(node.node_id.clone(), wih.to_string())) {
                    let via = t.lane.as_deref().map(|l| format!(", lane {l}")).unwrap_or_default();
                    let state = ticket_delivery_state(t.guarantee.as_deref());
                    out.push(Action::Wait {
                        line: format!(
                            "{}: waiting on vendor ticket {} for bot:{slug} ({wih}{via}, delivery {state})",
                            node.node_id, t.ticket
                        ),
                    });
                    continue;
                }
                let picked_for_bot = node.assignee.as_deref() == Some(format!("bot:{slug}").as_str());
                let open_refusal = node.wait_gates.iter().any(|g| {
                    g.kind == WaitGateKind::Manual
                        && g.params.get("source").and_then(Value::as_str) == Some(GATE_SOURCE)
                        && g.params.get("reason").and_then(Value::as_str) == Some(VENDOR_TICKET_FAILED)
                        && !g.is_resolved_ok()
                });
                if picked_for_bot && !open_refusal {
                    out.push(Action::VendorTicket { node_id: node.node_id.clone(), slug, wih: Some(wih.to_string()) });
                    continue;
                }
            }
            if !ready.contains(&node.node_id) {
                let deps_done = dag
                    .edges
                    .iter()
                    .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == node.node_id)
                    .all(|e| dag.nodes.get(&e.from_node_id).is_some_and(|n| n.status == "DONE"));
                if deps_done {
                    for g in node.blocking_wait_gates(now) {
                        let who = if g.kind == WaitGateKind::Manual { "needs you" } else { "waiting" };
                        let evidence = g
                            .params
                            .get(EVIDENCE_PARAM)
                            .and_then(Value::as_str)
                            .filter(|e| !g.description.contains(*e))
                            .map(|e| format!(" — look at: {e}"))
                            .unwrap_or_default();
                        out.push(Action::Wait {
                            line: format!(
                                "{}: {who} — {} gate {} \"{}\"{evidence} (resolve: allternit-factory workspace approve {}/{} {} --actor user:<you>)",
                                node.node_id, g.kind, g.gate_id, g.description, dag.dag_id, node.node_id, g.gate_id
                            ),
                        });
                    }
                }
                continue;
            }
            let Some(executor) = node.executor.clone() else {
                let what = if node.parent_node_id.is_none() {
                    "plan root is READY — verify and close it"
                } else {
                    "READY with no executor — pick up manually"
                };
                out.push(Action::Wait { line: format!("{}: {what}", node.node_id) });
                continue;
            };
            let (kind, name) = executor.split_once(':').unwrap_or(("", ""));
            if kind == "bot" {
                if self.opts.vendor_bots.contains_key(name) {
                    out.push(Action::VendorTicket { node_id: node.node_id.clone(), slug: name.to_string(), wih: None });
                } else {
                    out.push(Action::NotifyBot { node_id: node.node_id.clone(), slug: name.to_string() });
                }
                continue;
            }
            let harness = name.to_string();

            if let Some(wih) = node.current_wih_id.clone() {
                let Some(a) = latest else {
                    out.push(Action::Wait {
                        line: format!(
                            "{}: held by {} ({wih}) — not a drive attempt",
                            node.node_id,
                            node.assignee.as_deref().unwrap_or("?")
                        ),
                    });
                    continue;
                };
                // Open but not live here = interrupted (dry-run view; the live
                // path records it before deciding).
                let outcome = a.outcome.clone().unwrap_or_else(|| {
                    if a.exit_file().exists() { "exited".into() } else { "interrupted".into() }
                });
                if outcome == "exited" {
                    out.push(Action::Wait { line: format!("{}: attempt {} exited — will be collected", node.node_id, a.attempt_id) });
                    continue;
                }
                let acked = drive_gate_resolved(node, |p| {
                    p.get("attempt_id").and_then(Value::as_str) == Some(a.attempt_id.as_str())
                });
                let retry_safe = node.labels.iter().any(|l| l == RETRY_SAFE_LABEL);
                if outcome == "close_failed" && acked {
                    out.push(Action::Collect { attempt: a.clone() });
                } else if (outcome == "interrupted" && retry_safe) || acked {
                    out.push(Action::Spawn {
                        node_id: node.node_id.clone(),
                        harness,
                        wih: Some(wih),
                        restart_of: Some(a.attempt_id.clone()),
                    });
                } else {
                    let (reason, detail) = match outcome.as_str() {
                        "interrupted" => (
                            "interrupted",
                            format!(
                                "attempt {} was interrupted after its harness started; not restarting a node without `retry: safe`. Check its effects, then resolve this gate to restart it (or close {wih} yourself).",
                                a.attempt_id
                            ),
                        ),
                        "spawn_refused" => (
                            "harness_refused",
                            format!("spawn of {executor} was refused by the spawn gate (attempt {})", a.attempt_id),
                        ),
                        "close_failed" => (
                            "attempt_failed",
                            format!(
                                "attempt {} finished but `wih close` was refused; fix the cause, then resolve this gate to retry the close from its captured output",
                                a.attempt_id
                            ),
                        ),
                        other => (
                            "attempt_failed",
                            format!("attempt {} ended {other} without closing {wih}", a.attempt_id),
                        ),
                    };
                    out.push(Action::NeedsYou {
                        node_id: node.node_id.clone(),
                        reason,
                        detail,
                        executor: executor.clone(),
                        attempt_id: Some(a.attempt_id.clone()),
                    });
                }
                continue;
            }

            // Fresh pickup: admission pre-check, so a refused harness never
            // gets a WIH. Pickup always creates WIHs with leased writes.
            let refusal = if !self.cfg.harnesses.contains_key(&harness) {
                Some(("harness_unconfigured", format!(
                    "no argv for harness {harness} in .allternit/drive/config.json"
                )))
            } else {
                let argv0 = self.cfg.harnesses[&harness].argv[0].clone();
                let policy = WihPolicy { wih_id: "(pickup)".into(), requires_lease_for_write: Some(true), fence_strict: false };
                hook::admit(&argv0, Some(&policy)).err().map(|r| ("harness_refused", r))
            };
            match refusal {
                Some((reason, detail)) => {
                    let acked = drive_gate_resolved(node, |p| {
                        p.get("reason").and_then(Value::as_str) == Some(reason)
                            && p.get("executor").and_then(Value::as_str) == Some(executor.as_str())
                    });
                    if acked {
                        out.push(Action::Wait {
                            line: format!(
                                "{}: {executor} refusal acknowledged — pick it up manually or change its executor",
                                node.node_id
                            ),
                        });
                    } else {
                        out.push(Action::NeedsYou {
                            node_id: node.node_id.clone(),
                            reason,
                            detail,
                            executor,
                            attempt_id: None,
                        });
                    }
                }
                None => out.push(Action::Spawn {
                    node_id: node.node_id.clone(),
                    harness,
                    wih: None,
                    restart_of: None,
                }),
            }
        }
        out
    }

    /// Dry run: the same decision, printed, with no writes and no spawns.
    async fn plan(&mut self, report: &mut DriveReport) -> Result<()> {
        let (_, dag, attempts) = self.load().await?;
        let mut live = HashSet::new();
        for a in attempts.iter().filter(|a| a.outcome.is_none()) {
            if spawn::session_alive(&a.slug).await {
                live.insert(a.attempt_id.clone());
                report.plan.push(format!("{}: running {} (would adopt)", a.node_id, spawn::session_name(&a.slug)));
            }
        }
        let limits = self.limits();
        let counts = self.caps.peek(self.dag_id())?;
        let mut planned = 0usize;
        let capacity = Capacity::probe().admit(self.cfg.min_free_mem_mb, self.cfg.max_load_per_cpu);
        if let Err(reason) = &capacity {
            report.plan.push(format!("capacity: would refuse to start: {reason}"));
        }
        for route in route::plan_fail_routes(&dag) {
            report.plan.push(route.line(true));
        }
        let tickets = self.tickets().await?;
        for action in self.decide(&dag, &attempts, &live, &tickets) {
            let line = match action {
                Action::Wait { line } => line,
                Action::Collect { attempt } => format!(
                    "{}: would retry wih close from the captured output of attempt {}",
                    attempt.node_id, attempt.attempt_id
                ),
                Action::NotifyBot { node_id, slug } => {
                    format!("{node_id}: would mail bot:{slug} (dag:{} thread) and skip", self.dag_id())
                }
                Action::VendorTicket { node_id, slug, wih } => match (&wih, &self.vendor_api) {
                    (_, None) => format!(
                        "{node_id}: bot:{slug} is a vendor bot but no allternit-api client is configured; would add a needs-you gate ({VENDOR_TICKET_FAILED})"
                    ),
                    (Some(w), Some(_)) => format!("{node_id}: would create the vendor ticket for bot:{slug} on {w} (POST /api/v1/factory/node-tickets)"),
                    (None, Some(_)) => format!(
                        "{node_id}: would wih pickup for bot:{slug} + sign-open, then create its vendor ticket (POST /api/v1/factory/node-tickets)"
                    ),
                },
                Action::NeedsYou { node_id, reason, detail, .. } => {
                    format!("{node_id}: would add a needs-you gate ({reason}): {detail}")
                }
                Action::Spawn { node_id, harness, wih, restart_of } => {
                    if let Some(d) = would_defer(&limits, counts, planned) {
                        format!("{node_id}: would defer spawn: {d}")
                    } else {
                        planned += 1;
                        let argv = self
                            .cfg
                            .harness_argv(
                                &harness,
                                &ArgvVars {
                                    prompt: "<prompt>",
                                    prompt_file: "<run_dir>/prompt.md",
                                    wih_id: wih.as_deref().unwrap_or("<wih>"),
                                    dag_id: self.dag_id(),
                                    node_id: &node_id,
                                },
                            )
                            .unwrap_or_default();
                        let gated = hook::gate_argv(&argv, Some(Path::new("<session settings>")));
                        let step = match (&wih, &restart_of) {
                            (Some(w), Some(prev)) => format!("would restart on {w} (after {prev})"),
                            _ => "would wih pickup + sign-open".to_string(),
                        };
                        format!(
                            "{node_id}: {step}, spawn ao:{harness} [{}] --wih, capture output, wih close",
                            gated.join(" ")
                        )
                    }
                }
            };
            report.plan.push(line);
        }
        println!("drive {} --dry-run (no ledger writes, no spawns):", self.dag_id());
        println!(
            "  caps: dag {}/{} running, {}/{} spawns last hour; global {}/{} running, {}/{} last hour",
            counts.dag_running,
            limits.dag_concurrent,
            counts.dag_last_hour,
            limits.dag_per_hour,
            counts.global_running,
            limits.global_concurrent,
            counts.global_last_hour,
            limits.global_per_hour
        );
        for line in &report.plan {
            println!("  {line}");
        }
        Ok(())
    }

    /// Act on every failed node that carries an `on_fail` route: route it
    /// back (reopen the target and the nodes between, through the Gate) while
    /// rounds remain, else mark the root's closure degraded and hand the node
    /// to a person. Bounded: a node routes back at most `max_rounds` times.
    /// Returns status lines. A route the Gate refuses (e.g. the state moved
    /// under it) is reported and re-evaluated on the next pass.
    pub async fn route_failures(&self, report: &mut DriveReport) -> Result<Vec<String>> {
        let (_, dag, _) = self.load().await?;
        let mut lines = Vec::new();
        for r in route::plan_fail_routes(&dag) {
            let applied = match &r {
                FailRoute::Blocked { .. } => {
                    lines.push(r.line(false));
                    continue;
                }
                FailRoute::RouteBack { .. } => self.route_back(&dag, &r, report).await,
                FailRoute::Exhausted { .. } => self.rounds_exhausted(&dag, &r, report).await,
            };
            match applied {
                Ok(line) => lines.push(line),
                Err(err) => {
                    let line = format!("{}: on_fail route refused: {err:#}", r.node_id());
                    eprintln!("drive {}: {line}", self.dag_id());
                    lines.push(line);
                }
            }
        }
        Ok(lines)
    }

    async fn route_back(&self, dag: &DagState, r: &FailRoute, report: &mut DriveReport) -> Result<String> {
        let FailRoute::RouteBack { node_id, target, round, max_rounds, reopen } = r else {
            bail!("not a route-back");
        };
        let failed = dag.nodes.get(node_id).context("failed node vanished")?;
        let target_node = dag.nodes.get(target).context("on_fail target vanished")?;
        let why = format!("drive: on_fail route back from {node_id} (round {round}/{max_rounds})");
        let mut mutations: Vec<DagMutation> = reopen
            .iter()
            .map(|(n, from, to)| DagMutation::ChangeStatus {
                node_id: n.clone(),
                from: from.clone(),
                to: to.clone(),
                reason: Some(why.clone()),
            })
            .collect();
        let description = format!(
            "{}{}",
            target_node.description.as_deref().unwrap_or_default(),
            route::feedback_text(failed, *round, *max_rounds)
        );
        mutations.push(DagMutation::UpdateNode {
            node_id: target.clone(),
            patch: json!({ "description": description }),
        });
        mutations.push(DagMutation::SetState {
            node_id: node_id.clone(),
            dimension: route::ROUNDS_STATE.to_string(),
            value: round.to_string(),
            reason: Some(why.clone()),
        });
        let delta_id = self
            .gate()?
            .plan_refine(
                self.dag_id(),
                &format!("drive: route back {node_id} → {target} (round {round}/{max_rounds})"),
                "drive",
                mutations,
            )
            .await?;
        self.emit(
            ROUTE_BACK,
            json!({
                "dag_id": self.dag_id(),
                "node_id": node_id,
                "target_node_id": target,
                "round": round,
                "max_rounds": max_rounds,
                "failed_status": failed.status,
                "output_receipt_id": failed.output.as_ref().map(|o| o.receipt_id.clone()),
                "reopened": reopen.iter().map(|(n, f, t)| json!({ "node_id": n, "from": f, "to": t })).collect::<Vec<_>>(),
                "delta_id": delta_id,
            }),
        )
        .await?;
        let line = r.line(false);
        println!("drive {}: {line}", self.dag_id());
        report.routed_back.push((node_id.clone(), target.clone(), *round));
        Ok(line)
    }

    async fn rounds_exhausted(&self, dag: &DagState, r: &FailRoute, report: &mut DriveReport) -> Result<String> {
        let FailRoute::Exhausted { node_id, target, max_rounds, reopen, root_id, closure_text } = r else {
            bail!("not a rounds-exhausted route");
        };
        let failed = dag.nodes.get(node_id).context("failed node vanished")?;
        let why = format!("drive: {node_id} used {max_rounds}/{max_rounds} on_fail rounds");
        let mut mutations = Vec::new();
        if let Some(root) = root_id {
            mutations.push(DagMutation::SetState {
                node_id: root.clone(),
                dimension: CLOSURE_STATE.to_string(),
                value: route::CLOSURE_DEGRADED.to_string(),
                reason: Some(closure_text.clone()),
            });
        }
        // Reopen first (a wait-gate cannot be added to a closed node); the
        // needs-you gate below keeps it from running until a person decides.
        if reopen.1 != reopen.2 {
            mutations.push(DagMutation::ChangeStatus {
                node_id: reopen.0.clone(),
                from: reopen.1.clone(),
                to: reopen.2.clone(),
                reason: Some(why.clone()),
            });
        }
        self.gate()?
            .plan_refine(
                self.dag_id(),
                &format!("drive: rounds exhausted on {node_id} → degraded"),
                "drive",
                mutations,
            )
            .await?;
        let output = failed
            .output
            .as_ref()
            .map(|o| format!(" Its last output: {}.", o.output_path))
            .unwrap_or_default();
        let detail = format!(
            "{node_id} closed {} again after {max_rounds} route-back round(s) to {target}; rounds exhausted, \
             the flow is degraded ({closure_text}).{output} Resolve this gate to run it once more, or close the plan root yourself.",
            failed.status
        );
        let executor = failed.executor.clone().unwrap_or_default();
        let gate_id = self
            .needs_you(node_id, route::ROUNDS_EXHAUSTED_REASON, &detail, &executor, None)
            .await?;
        self.emit(
            ROUNDS_EXHAUSTED,
            json!({
                "dag_id": self.dag_id(),
                "node_id": node_id,
                "target_node_id": target,
                "max_rounds": max_rounds,
                "root_node_id": root_id,
                "closure": route::CLOSURE_DEGRADED,
                "closure_text": closure_text,
                "gate_id": gate_id,
            }),
        )
        .await?;
        report.needs_you.push((node_id.clone(), route::ROUNDS_EXHAUSTED_REASON.to_string()));
        report.degraded.push(node_id.clone());
        Ok(format!("{node_id}: needs you [{gate_id}] {detail}"))
    }

    fn running_entry(&self, node_id: &str, slug: &str) -> RunningEntry {
        RunningEntry {
            dag_id: self.dag_id().to_string(),
            node_id: node_id.to_string(),
            slug: slug.to_string(),
            reserved_at: Utc::now().to_rfc3339(),
            pid: std::process::id(),
        }
    }

    async fn start(
        &mut self,
        dag: &DagState,
        node_id: &str,
        harness: &str,
        wih: Option<String>,
        restart_of: Option<String>,
        report: &mut DriveReport,
    ) -> Result<Started> {
        let dag_id = self.dag_id().to_string();
        let node = dag.nodes.get(node_id).context("node vanished")?.clone();
        let executor = format!("ao:{harness}");

        if let Err(reason) = Capacity::probe().admit(self.cfg.min_free_mem_mb, self.cfg.max_load_per_cpu) {
            let d = Deferral { kind: "capacity", limit: 0, current: 0 };
            self.log_deferral(node_id, &d, Some(&reason)).await?;
            report.deferred.push((node_id.to_string(), format!("capacity: {reason}")));
            return Ok(Started::Deferred(format!("capacity: {reason}")));
        }

        let attempt_no = self.load().await?.2.iter().filter(|a| a.node_id == node_id).count() + 1;
        let slug = format!("drive-{dag_id}-{node_id}-{attempt_no}");
        match self
            .caps
            .try_reserve(self.running_entry(node_id, &slug), self.limits(), &session_alive_blocking)?
        {
            Ok(()) => {}
            Err(d) => {
                self.log_deferral(node_id, &d, None).await?;
                report.deferred.push((node_id.to_string(), d.to_string()));
                return Ok(Started::Deferred(d.to_string()));
            }
        }

        // Gate 1 + open-sign (fresh), or reuse the WIH (restart).
        let gate = self.gate()?.clone();
        let (wih_id, prompt_body) = match wih {
            Some(w) => {
                let text = wih_prompt_text(&self.ledger, &w).await?.or(node.description.clone());
                (w, text)
            }
            None => {
                let pickup = gate
                    .wih_pickup_detailed(
                        &dag_id,
                        node_id,
                        &format!("drive-{harness}"),
                        WihPickupOptions { role: node.owner_role.clone(), fresh: false },
                    )
                    .await;
                let pickup = match pickup {
                    Ok(p) => p,
                    Err(err) => {
                        self.caps.release(&slug, &session_alive_blocking)?;
                        let detail = match GateError::from_anyhow(&err) {
                            Some(g) => format!("Gate 1 refused pickup: {} ({})", g.reason, g.code),
                            None => format!("pickup failed: {err:#}"),
                        };
                        let gate_id = self.needs_you(node_id, "pickup_refused", &detail, &executor, None).await?;
                        report.needs_you.push((node_id.to_string(), "pickup_refused".into()));
                        return Ok(Started::NeedsYou(format!("{node_id}: needs you [{gate_id}] {detail}")));
                    }
                };
                gate.wih_sign_open(&pickup.wih_id, &format!("drive:{}:{executor}", std::process::id()))
                    .await?;
                let text = pickup.resolved_description.clone().or(node.description.clone());
                (pickup.wih_id, text)
            }
        };

        let attempt_id = format!("{wih_id}-a{attempt_no}");
        let run_dir = drive_dir(&self.root)
            .join("runs")
            .join(&dag_id)
            .join(node_id)
            .join(&attempt_id);
        ensure_dir(&run_dir)?;
        let prompt = build_prompt(&dag_id, &node, &wih_id, prompt_body.as_deref());
        let prompt_file = run_dir.join("prompt.md");
        std::fs::write(&prompt_file, &prompt)?;
        let argv = self
            .cfg
            .harness_argv(
                harness,
                &ArgvVars {
                    prompt: &prompt,
                    prompt_file: &prompt_file.to_string_lossy(),
                    wih_id: &wih_id,
                    dag_id: &dag_id,
                    node_id,
                },
            )
            .context("harness argv vanished from config")?;
        let timeout = self.timeout_seconds();

        // The attempt is recorded before the spawn: from here on the harness
        // may act, so a crash leaves an open attempt that reads as interrupted.
        self.emit(
            ATTEMPT_STARTED,
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "wih_id": wih_id,
                "attempt_id": attempt_id,
                "attempt": attempt_no,
                "executor": executor,
                "harness": harness,
                "slug": slug,
                "run_dir": run_dir,
                "timeout_seconds": timeout,
                "restart_of": restart_of,
                "pid": std::process::id(),
            }),
        )
        .await?;
        let attempt = Attempt {
            attempt_id: attempt_id.clone(),
            node_id: node_id.to_string(),
            wih_id: wih_id.clone(),
            executor: executor.clone(),
            slug: slug.clone(),
            run_dir: run_dir.clone(),
            started_at: Utc::now(),
            timeout_seconds: timeout,
            outcome: None,
        };

        // Same admission the spawn path runs, against the WIH's real policy,
        // so a refusal is recorded as such (not as a generic spawn failure).
        let policy = hook::load_wih_policy(&self.ledger, &wih_id).await?;
        let refused = hook::admit(&argv[0], Some(&policy)).err();
        let capture = CaptureFiles {
            stdout: run_dir.join("stdout.txt"),
            stderr: run_dir.join("stderr.txt"),
            exit_code: run_dir.join("exit_code"),
        };
        let workdir = self.opts.workdir.clone().unwrap_or_else(|| self.root.clone());
        let spawned = match refused {
            Some(reason) => Err((true, reason)),
            None => self
                .orch()?
                .spawn(SpawnOptions {
                    slug: &slug,
                    repo: &workdir,
                    cmd: &argv,
                    worktree: false,
                    vendor: harness,
                    mode: "headless",
                    task_file: Some(&prompt_file),
                    notes_sentinel: None,
                    bot: None,
                    wih: Some(&wih_id),
                    capture: Some(&capture),
                })
                .await
                .map(|_| ())
                .map_err(|e| (false, format!("{e:#}"))),
        };
        if let Err((was_refusal, reason)) = spawned {
            self.caps.release(&slug, &session_alive_blocking)?;
            let outcome = if was_refusal { "spawn_refused" } else { "spawn_failed" };
            if was_refusal {
                let _ = self
                    .ledger
                    .append(hook::spawn_refused_event(&argv[0], &wih_id, &reason))
                    .await;
            }
            self.record_finish(&attempt, outcome, None, None, Some(&reason)).await?;
            let why = if was_refusal { "harness_refused" } else { "attempt_failed" };
            let detail = format!("{outcome} for {executor} (attempt {attempt_id}): {reason}");
            let gate_id = self
                .needs_you(node_id, why, &detail, &executor, Some(&attempt_id))
                .await?;
            report.needs_you.push((node_id.to_string(), why.to_string()));
            return Ok(Started::NeedsYou(format!("{node_id}: needs you [{gate_id}] {detail}")));
        }

        println!(
            "drive {dag_id}: spawned {node_id} ({executor}) wih {wih_id} attempt {attempt_id} session {}",
            spawn::session_name(&slug)
        );
        let aref = attempt.as_ref(&dag_id);
        if let Err(e) = self.hooks.on_attempt_started(&aref).await {
            eprintln!("drive: attempt-started hook failed for {node_id}: {e:#}");
        }
        report.spawned.push((node_id.to_string(), attempt_id));
        let deadline = attempt.started_at + chrono::Duration::seconds(timeout as i64);
        self.running.insert(node_id.to_string(), Running { attempt, deadline });
        Ok(Started::Spawned)
    }

    /// Collect a finished session's output, `wih close` it, record the finish.
    async fn finish(&mut self, attempt: Attempt, outcome: WatchOutcome, report: &mut DriveReport) -> Result<()> {
        let dag_id = self.dag_id().to_string();
        let stdout = read_capped(&attempt.run_dir.join("stdout.txt"));
        let stderr = read_capped(&attempt.run_dir.join("stderr.txt"));
        let (label, status, exit_code) = match outcome {
            WatchOutcome::Done => {
                let code = std::fs::read_to_string(attempt.exit_file())
                    .ok()
                    .and_then(|s| s.trim().parse::<i32>().ok());
                if code == Some(0) {
                    ("done", "DONE", code)
                } else {
                    ("failed", "FAILED", code)
                }
            }
            WatchOutcome::Dead => ("dead", "FAILED", None),
            WatchOutcome::Timeout => {
                let _ = self.orch()?.kill(&attempt.slug, false).await;
                ("timeout", "FAILED", None)
            }
        };
        let mut evidence = vec![format!("drive:attempt:{}:{label}", attempt.attempt_id)];
        if let Some(code) = exit_code {
            evidence.push(format!("drive:exit_code:{code}"));
        }
        let output: Option<String> = if status == "DONE" {
            (!stdout.trim().is_empty()).then(|| stdout.clone())
        } else {
            let why = match label {
                "failed" => format!("harness exited {}", exit_code.map_or("?".into(), |c| c.to_string())),
                "dead" => "session died without an exit code".to_string(),
                _ => format!("timed out after {}s; session killed", attempt.timeout_seconds),
            };
            Some(format!(
                "drive attempt {} {label}: {why}\n\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}\n",
                attempt.attempt_id
            ))
        };

        let close = self
            .gate()?
            .wih_close_with(&attempt.wih_id, status, &evidence, output.as_deref())
            .await;
        let (final_outcome, receipt) = match close {
            Ok(receipt) => {
                self.record_finish(&attempt, label, exit_code, receipt.as_deref(), None).await?;
                println!(
                    "drive {dag_id}: {} {label} -> {status}{}",
                    attempt.node_id,
                    receipt.as_deref().map(|r| format!(" (output receipt {r})")).unwrap_or_default()
                );
                (label.to_string(), receipt)
            }
            Err(err) => {
                let reason = format!("wih close {status} refused: {err:#}");
                eprintln!("drive {dag_id}: {} {reason}", attempt.node_id);
                self.record_finish(&attempt, "close_failed", exit_code, None, Some(&reason)).await?;
                ("close_failed".to_string(), None)
            }
        };
        let _ = self.orch()?.kill(&attempt.slug, false).await;
        self.caps.release(&attempt.slug, &session_alive_blocking)?;
        let finished = NodeFinished {
            attempt: attempt.as_ref(&dag_id),
            outcome: final_outcome.clone(),
            close_status: status.to_string(),
            receipt_id: receipt,
        };
        if let Err(e) = self.hooks.on_node_finished(&finished).await {
            eprintln!("drive: node-finished hook failed for {}: {e:#}", attempt.node_id);
        }
        report.finished.push((attempt.node_id.clone(), final_outcome));
        Ok(())
    }

    async fn record_finish(
        &self,
        a: &Attempt,
        outcome: &str,
        exit_code: Option<i32>,
        receipt_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<()> {
        self.emit(
            ATTEMPT_FINISHED,
            json!({
                "dag_id": self.dag_id(),
                "node_id": a.node_id,
                "wih_id": a.wih_id,
                "attempt_id": a.attempt_id,
                "outcome": outcome,
                "exit_code": exit_code,
                "receipt_id": receipt_id,
                "reason": reason,
            }),
        )
        .await
    }

    /// Hand a node to a human: a manual wait-gate (source `drive`) plus a
    /// `DriveNeedsYou` event. The gate keeps the node out of `ready_nodes`.
    async fn needs_you(
        &self,
        node_id: &str,
        reason: &str,
        detail: &str,
        executor: &str,
        attempt_id: Option<&str>,
    ) -> Result<String> {
        let mut params = HashMap::new();
        params.insert("source".to_string(), json!(GATE_SOURCE));
        params.insert("reason".to_string(), json!(reason));
        params.insert("executor".to_string(), json!(executor));
        if let Some(a) = attempt_id {
            params.insert("attempt_id".to_string(), json!(a));
        }
        let gate_id = self
            .gate()?
            .add_node_wait_gate(
                self.dag_id(),
                node_id,
                WaitGateKind::Manual,
                Some(format!("drive: {detail}")),
                params,
                "drive",
            )
            .await?;
        self.emit(
            NEEDS_YOU,
            json!({
                "dag_id": self.dag_id(),
                "node_id": node_id,
                "reason": reason,
                "gate_id": gate_id,
                "executor": executor,
                "attempt_id": attempt_id,
                "detail": detail,
            }),
        )
        .await?;
        println!("drive {}: {node_id} needs you ({reason}) — gate {gate_id}: {detail}", self.dag_id());
        let item = NeedsYou {
            dag_id: self.dag_id().to_string(),
            node_id: node_id.to_string(),
            reason: reason.to_string(),
            gate_id: gate_id.clone(),
            detail: detail.to_string(),
        };
        if let Err(e) = self.hooks.on_needs_you(&item).await {
            eprintln!("drive: needs-you hook failed for {node_id}: {e:#}");
        }
        Ok(gate_id)
    }

    /// The vendor bot slug of a `bot:<slug>` node, when `--team` named it a vendor bot.
    fn vendor_slug(&self, node: &DagNode) -> Option<String> {
        let slug = node.executor.as_deref()?.strip_prefix("bot:")?;
        self.opts.vendor_bots.contains_key(slug).then(|| slug.to_string())
    }

    /// Deliver a node to a vendor bot: Gate 1 pickup for `bot:<slug>` (unless
    /// `wih` is the WIH already held for it) + open-sign, then
    /// `POST /api/v1/factory/node-tickets`, then `DriveVendorTicketCreated`.
    /// A refusal anywhere is a needs-you gate carrying the fact; nothing falls
    /// back to mail. Returns the status line.
    async fn vendor_ticket(
        &self,
        dag: &DagState,
        node_id: &str,
        slug: &str,
        wih: Option<String>,
        report: &mut DriveReport,
    ) -> Result<String> {
        let dag_id = self.dag_id().to_string();
        let executor = format!("bot:{slug}");
        let node = dag.nodes.get(node_id).context("node vanished")?.clone();
        let Some(api) = self.vendor_api.clone() else {
            let detail = format!(
                "bot:{slug} is a vendor bot but drive has no allternit-api client (ALLTERNIT_API_URL is not set), so no ticket can be created; nothing was picked up"
            );
            let gate_id = self.needs_you(node_id, VENDOR_TICKET_FAILED, &detail, &executor, None).await?;
            report.needs_you.push((node_id.to_string(), VENDOR_TICKET_FAILED.to_string()));
            return Ok(format!("{node_id}: needs you [{gate_id}] {detail}"));
        };
        let gate = self.gate()?.clone();
        let (wih_id, body) = match wih {
            Some(w) => {
                let text = wih_prompt_text(&self.ledger, &w).await?.or(node.description.clone());
                (w, text)
            }
            None => {
                let pickup = gate
                    .wih_pickup_detailed(
                        &dag_id,
                        node_id,
                        &executor,
                        WihPickupOptions { role: node.owner_role.clone(), fresh: false },
                    )
                    .await;
                let pickup = match pickup {
                    Ok(p) => p,
                    Err(err) => {
                        let detail = match GateError::from_anyhow(&err) {
                            Some(g) => format!("Gate 1 refused pickup for {executor}: {} ({})", g.reason, g.code),
                            None => format!("pickup for {executor} failed: {err:#}"),
                        };
                        let gate_id = self.needs_you(node_id, "pickup_refused", &detail, &executor, None).await?;
                        report.needs_you.push((node_id.to_string(), "pickup_refused".into()));
                        return Ok(format!("{node_id}: needs you [{gate_id}] {detail}"));
                    }
                };
                gate.wih_sign_open(&pickup.wih_id, &format!("drive:{}:{executor}", std::process::id()))
                    .await?;
                let text = pickup.resolved_description.clone().or(node.description.clone());
                (pickup.wih_id, text)
            }
        };
        let instructions = build_prompt(&dag_id, &node, &wih_id, body.as_deref());
        let (root, title) = (self.root.clone(), node.title.clone());
        let (s, d, n, w) = (slug.to_string(), dag_id.clone(), node_id.to_string(), wih_id.clone());
        let created = tokio::task::spawn_blocking(move || {
            vendor_ticket_for_node(api.as_ref(), &s, &d, &n, &w, &root, &title, &instructions)
        })
        .await
        .map_err(|e| anyhow!("vendor ticket task failed: {e}"))?;
        let ticket = match created {
            Ok(t) => t,
            Err(e) => {
                let detail = format!(
                    "vendor ticket for {executor} was not created ({}): {}. The node stays picked up on {wih_id}; fix the cause, then resolve this gate to create the ticket again",
                    e.code, e.fact
                );
                let gate_id = self.needs_you(node_id, VENDOR_TICKET_FAILED, &detail, &executor, None).await?;
                report.needs_you.push((node_id.to_string(), VENDOR_TICKET_FAILED.to_string()));
                return Ok(format!("{node_id}: needs you [{gate_id}] {detail}"));
            }
        };
        let state = ticket_delivery_state(ticket.guarantee.as_deref());
        let team = self.opts.vendor_bots.get(slug).and_then(|v| v.team.clone());
        self.emit(
            VENDOR_TICKET_CREATED,
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "wih_id": wih_id,
                "executor": executor,
                "to": team.as_deref().map(|t| format!("{slug}@{t}")).unwrap_or_else(|| executor.clone()),
                "ticket": ticket.ticket,
                "lane": ticket.lane,
                "guarantee": ticket.guarantee,
                "created": ticket.created,
                "nudge_sent": ticket.nudge_sent,
            }),
        )
        .await?;
        report.vendor_tickets.push((node_id.to_string(), ticket.ticket.clone(), state.to_string()));
        let lane = ticket.lane.as_deref().map(|l| format!(", lane {l}")).unwrap_or_default();
        println!(
            "drive {dag_id}: {node_id} delivery via vendor_ticket {} to {executor} ({wih_id}{lane}, state {state})",
            ticket.ticket
        );
        if ticket.guarantee.is_none() {
            let detail = format!(
                "vendor ticket {} for {executor} has no lane that can take it, so it was not sent; connect the vendor bot's account, then resolve this gate",
                ticket.ticket
            );
            let gate_id = self.needs_you(node_id, "vendor_ticket_no_lane", &detail, &executor, None).await?;
            report.needs_you.push((node_id.to_string(), "vendor_ticket_no_lane".into()));
            return Ok(format!("{node_id}: needs you [{gate_id}] {detail}"));
        }
        Ok(format!(
            "{node_id}: delivered to {executor} as vendor ticket {} ({wih_id}{lane}, delivery {state})",
            ticket.ticket
        ))
    }

    /// Mail the bot once per node (idempotent via `DriveBotNotified`).
    async fn notify_bot(&self, dag: &DagState, node_id: &str, slug: &str) -> Result<()> {
        let events = self
            .ledger
            .query(LedgerQuery { r#type: Some(BOT_NOTIFIED.to_string()), ..Default::default() })
            .await?;
        let already = events.iter().any(|e| {
            e.payload.get("dag_id").and_then(Value::as_str) == Some(self.dag_id())
                && e.payload.get("node_id").and_then(Value::as_str) == Some(node_id)
        });
        if already {
            return Ok(());
        }
        let node = dag.nodes.get(node_id).context("node vanished")?;
        let mail = Mail::new(MailOptions {
            root_dir: Some(self.root.clone()),
            ledger: self.ledger.clone(),
            actor_id: Some("drive".to_string()),
            actor_type: Some(ActorType::Agent),
            mail_index: None,
        });
        let thread = mail.ensure_thread(&format!("dag:{}", self.dag_id())).await?;
        let body = format!(
            "Node `{node_id}` (\"{}\") in `dag:{}` is READY and assigned to `bot:{slug}`.\n\n\
             `drive` does not spawn bots. Pick it up with:\n\n    allternit-factory workspace node claim {node_id} --dag {} --agent bot:{slug}\n",
            node.title,
            self.dag_id(),
            self.dag_id()
        );
        let message_id = mail
            .send_typed_message(
                &thread,
                TypedMessage {
                    from_agent: "drive".to_string(),
                    to_agents: vec![format!("bot:{slug}")],
                    subject: Some(format!("Ready for bot:{slug}: {}", node.title)),
                    importance: MailImportance::Normal,
                    ack_required: true,
                    body,
                },
            )
            .await?;
        self.emit(
            BOT_NOTIFIED,
            json!({
                "dag_id": self.dag_id(),
                "node_id": node_id,
                "executor": format!("bot:{slug}"),
                "thread_id": thread,
                "message_id": message_id,
            }),
        )
        .await?;
        println!("drive {}: {node_id} is for bot:{slug}; mailed {thread} ({message_id})", self.dag_id());
        Ok(())
    }

    async fn log_deferral(&mut self, node_id: &str, d: &Deferral, detail: Option<&str>) -> Result<()> {
        if !self.deferral_logged.insert((node_id.to_string(), d.kind)) {
            return Ok(());
        }
        self.emit(
            SPAWN_DEFERRED,
            json!({
                "dag_id": self.dag_id(),
                "node_id": node_id,
                "reason": d.kind,
                "limit": d.limit,
                "current": d.current,
                "detail": detail,
            }),
        )
        .await
    }

    async fn emit(&self, event_type: &str, payload: Value) -> Result<()> {
        self.ledger
            .append(AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: Actor { r#type: ActorType::Agent, id: "drive".to_string() },
                scope: None,
                r#type: event_type.to_string(),
                payload,
                provenance: None,
            })
            .await?;
        Ok(())
    }
}

enum Started {
    Spawned,
    Deferred(String),
    NeedsYou(String),
}

/// True when the node has a drive-created manual gate matching `pred` that
/// was resolved ok/skipped.
fn drive_gate_resolved(node: &DagNode, pred: impl Fn(&HashMap<String, Value>) -> bool) -> bool {
    node.wait_gates.iter().any(|g| {
        g.kind == WaitGateKind::Manual
            && g.params.get("source").and_then(Value::as_str) == Some(GATE_SOURCE)
            && g.is_resolved_ok()
            && pred(&g.params)
    })
}

fn would_defer(limits: &CapLimits, counts: CapCounts, planned: usize) -> Option<Deferral> {
    let checks = [
        ("global_max_concurrent", limits.global_concurrent, counts.global_running + planned),
        ("max_concurrent", limits.dag_concurrent, counts.dag_running + planned),
        ("global_max_spawns_per_hour", limits.global_per_hour, counts.global_last_hour + planned),
        ("max_spawns_per_hour", limits.dag_per_hour, counts.dag_last_hour + planned),
    ];
    checks
        .into_iter()
        .find(|(_, limit, current)| current >= limit)
        .map(|(kind, limit, current)| Deferral { kind, limit, current })
}

/// The WIH's resolved prompt (Gate 1 output-placeholder resolution), if any.
async fn wih_prompt_text(ledger: &Ledger, wih_id: &str) -> Result<Option<String>> {
    let events = ledger
        .query(LedgerQuery { r#type: Some("WIHCreated".to_string()), ..Default::default() })
        .await?;
    Ok(events
        .iter()
        .find(|e| e.payload.get("wih_id").and_then(Value::as_str) == Some(wih_id))
        .and_then(|e| e.payload.get("resolved_prompt_path").and_then(Value::as_str))
        .and_then(|p| std::fs::read_to_string(p).ok()))
}

fn build_prompt(dag_id: &str, node: &DagNode, wih_id: &str, body: Option<&str>) -> String {
    format!(
        "# {title}\n\n{body}\n\n---\nCommRails: dag {dag_id}, node {node_id}, WIH {wih_id}. \
         Your final answer on stdout becomes this node's output (`wih close --output`). \
         Writes need a lease: `allternit-factory internal rails lease request {wih_id} <agent> <paths...>`.\n",
        title = node.title,
        body = body.unwrap_or("(no description)"),
        node_id = node.node_id,
    )
}

/// Up to the last 16 KiB of a capture file.
fn read_capped(path: &Path) -> String {
    const CAP: usize = 16 * 1024;
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if text.len() <= CAP {
        return text;
    }
    let mut start = text.len() - CAP;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[... truncated ...]\n{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn would_defer_counts_planned_spawns() {
        let limits = CapLimits { dag_concurrent: 2, dag_per_hour: 20, global_concurrent: 4, global_per_hour: 20 };
        let counts = CapCounts { dag_running: 1, ..Default::default() };
        assert!(would_defer(&limits, counts, 0).is_none());
        assert_eq!(would_defer(&limits, counts, 1).unwrap().kind, "max_concurrent");
    }

    #[test]
    fn read_capped_keeps_the_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x");
        std::fs::write(&p, "a".repeat(20_000) + "END").unwrap();
        let s = read_capped(&p);
        assert!(s.ends_with("END"));
        assert!(s.len() < 17_000);
    }
}
