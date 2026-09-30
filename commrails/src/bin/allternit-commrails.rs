use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use allternit_commrails::bus::{Bus, BusMessage, BusOptions, NewBusMessage};
use allternit_commrails::cli::judge::{
    run_judge_command, run_leases_command, JudgeCmd, JudgeContext, LeasesCmd,
};
use allternit_commrails::cli::work::{run_work_command, WorkCmd, WorkContext};
use allternit_commrails::core::ids::{create_event_id, create_lease_id};
use allternit_commrails::core::io::{ensure_dir, write_json_atomic};
use allternit_commrails::dependencies::load_graph;
use allternit_commrails::gate::gate::{GateOptions, WihPickupOptions};
use allternit_commrails::gate::GateError;
use allternit_commrails::templates::{parse_param_args, plan_from_template, TemplateStore};
use allternit_commrails::wait_gates::{GateOutcome, WaitGateKind};
use allternit_commrails::work::needs_you::pending_manual_gates;
use allternit_commrails::leases::leases::LeasesOptions;
use allternit_commrails::ledger::ledger::LedgerOptions;
use allternit_commrails::graph::{views, GraphAnalytics, GraphView, InsightsConfig};
use allternit_commrails::policy;
use allternit_commrails::rails_id::TicketId;
use allternit_commrails::tickets::{self, TicketStore};
use allternit_commrails::wait_gates::WaitGateStore;
use allternit_commrails::wih::projection::project_wih;
use allternit_commrails::wih::types::LoopPolicy;
use allternit_commrails::work::graph::{has_cycle_edges, ready_nodes};
use allternit_commrails::work::projection::project_dag;
use allternit_commrails::work::types::DagState;
use allternit_commrails::{
    AllternitEvent, Actor, ActorType, DagMutation, EventScope, Gate, Index, IndexOptions, LeaseRequest,
    Leases, Ledger, LedgerQuery, Mail, MailOptions, Orchestrator, PeerEnvelope, PeerRegistry,
    ReceiptStore, ReceiptStoreOptions, SpawnOptions, Steer, Vault, VaultOptions, WatchOutcome,
    WorkOps, send_envelope,
};
#[cfg(unix)]
use allternit_commrails::PeerSocket;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::{sleep, Duration as TokioDuration};

#[derive(Parser)]
#[command(name = "allternit-commrails")]
#[command(about = "Allternit CommRails CLI", long_about = None)]
struct Cli {
    #[arg(long)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(subcommand)]
    Plan(PlanCmd),
    #[command(subcommand)]
    Node(NodeCmd),
    #[command(subcommand)]
    Dag(DagCmd),
    #[command(subcommand)]
    Wih(WihCmd),
    #[command(subcommand)]
    Lease(LeaseCmd),
    #[command(subcommand)]
    Ledger(LedgerCmd),
    #[command(subcommand)]
    Index(IndexCmd),
    #[command(subcommand)]
    Mail(MailCmd),
    #[command(subcommand)]
    Gate(GateCmd),
    #[command(subcommand)]
    Vault(VaultCmd),
    #[command(subcommand)]
    Runner(RunnerCmd),
    #[command(subcommand)]
    Bus(BusCmd),
    #[command(subcommand)]
    Transport(TransportCmd),
    Init,
    #[command(subcommand)]
    Work(WorkCmd),
    #[command(subcommand)]
    Ticket(TicketCmd),
    #[command(subcommand)]
    Graph(GraphCmd),
    #[command(subcommand)]
    Peer(PeerCmd),
    #[command(subcommand)]
    Orchestrator(OrchestratorCmd),
    #[command(subcommand)]
    Steer(SteerCmd),
    /// Spawn-gate hooks injected into third-party harnesses (Gate 2 + hard floor).
    #[command(subcommand)]
    Hook(HookCmd),
    /// Node-scoped wait-gates on WIH DAG nodes.
    #[command(subcommand, name = "wait-gate")]
    WaitGate(WaitGateCmd),
    /// Fail-closed judge: node verdicts, tool decisions, policy.
    #[command(subcommand)]
    Judge(JudgeCmd),
    /// Lease-holder heartbeats and stale-lease reclaim.
    #[command(subcommand)]
    Leases(LeasesCmd),
}

#[derive(Subcommand)]
enum HookCmd {
    /// Claude Code PreToolUse hook: reads the hook JSON on stdin, prints a
    /// deny decision (or nothing, to allow). Always exits 0; every internal
    /// error is a deny, so a broken gate never fails open.
    ClaudePretool {
        /// WIH the spawned session is bound to. Without it only the hard floor applies.
        #[arg(long, env = "ALLTERNIT_COMMRAILS_WIH")]
        wih: Option<String>,
        /// Harness label recorded on ledger events.
        #[arg(long, default_value = "claude-code")]
        harness: String,
        /// Directory the harness works in (lease paths are relative to it).
        /// Defaults to --root.
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// Print the session-scoped Claude Code settings (for `claude --settings`).
    ClaudeSettings {
        #[arg(long, env = "ALLTERNIT_COMMRAILS_WIH")]
        wih: Option<String>,
        /// Directory the harness works in. Defaults to --root.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Spawn admission: exit 0 when `harness` may run (bound to `wih` if
    /// given), exit 3 with the reason on stderr when it must be refused.
    SpawnCheck {
        #[arg(long)]
        harness: String,
        #[arg(long)]
        wih: Option<String>,
    },
}

#[derive(Subcommand)]
enum WaitGateCmd {
    /// Attach a wait-gate to a node (Gate 0 refine: prompt delta provenance).
    Add {
        /// `<dag_id>/<node_id>`
        #[arg(long)]
        node: String,
        kind: WaitGateKind,
        #[arg(long)]
        description: Option<String>,
        /// Timer gates: RFC 3339 timestamp.
        #[arg(long)]
        until: Option<String>,
        /// GitHub gates: owner/repo.
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
    },
    /// Resolve a node wait-gate. Manual gates require --actor.
    Resolve {
        /// `<dag_id>/<node_id>`
        #[arg(long)]
        node: String,
        gate_id: String,
        #[arg(long, default_value = "ok")]
        outcome: GateOutcome,
        /// `user:<id>` or `agent:<id>` (bare id = user).
        #[arg(long)]
        actor: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// List wait-gates on one node (`--node`) or every node of a dag (`--dag`).
    List {
        #[arg(long)]
        node: Option<String>,
        #[arg(long = "dag")]
        dag_id: Option<String>,
    },
    /// Unresolved manual node gates across all dags ("needs you").
    Pending {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum GraphCmd {
    /// Health summary, keystones, bottlenecks, and quick wins over the
    /// ticket dependency graph.
    Insights,
    /// Ranked ready tickets with scores, reasons, and unblock counts.
    Triage,
    /// What a ticket affects: direct + transitive dependents, critical-path
    /// impact, centrality ranks.
    Impact {
        ticket_id: String,
    },
}

#[derive(Subcommand)]
enum PeerCmd {
    /// Register this session as a local peer and print its id + inbox socket.
    Register {
        name: String,
        #[arg(long, default_value = "agent")]
        vendor: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Remove a peer from the registry.
    Unregister {
        id_or_name: String,
    },
    /// List local peers.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Send a heartbeat for a peer.
    Heartbeat {
        id_or_name: String,
    },
    /// Listen on a peer's inbox socket and print incoming envelopes. (Unix only)
    #[cfg(unix)]
    Inbox {
        id_or_name: String,
    },
    /// Send a plain-text message to a peer's inbox.
    Send {
        to: String,
        message: String,
    },
}

#[derive(Subcommand)]
enum OrchestratorCmd {
    /// Spawn a tmux session for an agent and register it as a Rails peer.
    Spawn {
        slug: String,
        repo: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
        #[arg(long)]
        worktree: bool,
        #[arg(long, default_value = "agent")]
        vendor: String,
        #[arg(long, default_value = "interactive")]
        mode: String,
        #[arg(long)]
        task_file: Option<PathBuf>,
        #[arg(long)]
        notes_sentinel: Option<PathBuf>,
        /// Bind the session to a WIH: hooked harnesses (claude) enforce Gate 2
        /// against it; unhooked harnesses are refused if its policy needs leased writes.
        #[arg(long, env = "ALLTERNIT_COMMRAILS_WIH")]
        wih: Option<String>,
    },
    /// Send data to a running executor session.
    Send {
        slug: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        data: Vec<String>,
    },
    /// Block until the sentinel file exists, the pane dies, or timeout.
    Watch {
        slug: String,
        sentinel: PathBuf,
        #[arg(long, default_value_t = 3600)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 20)]
        interval_seconds: u64,
    },
    /// Show status for all ao-* sessions or one specific session.
    Status {
        slug: Option<String>,
        #[arg(long, default_value_t = 25)]
        lines: usize,
    },
    /// Kill a session and unregister its peer.
    Kill {
        slug: String,
        #[arg(long)]
        rm_worktree: bool,
    },
    /// Verify the delegation toolchain.
    Doctor,
}

#[derive(Subcommand)]
enum SteerCmd {
    /// Hash .steering/checkpoint.md and emit a SteeringCheckpoint ledger event
    /// when it changed. Prints the hash and whether it changed.
    Checkpoint {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Build steering context and consult the configured steering agent.
    /// Prints the raw answer; first line is APPROVE or STEER.
    Consult {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        prompt_file: Option<PathBuf>,
    },
    /// Run a commit-gate consult. Exits 0 on APPROVE, 2 on STEER.
    CommitGate {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TicketCmd {
    /// List tickets that are ready: open, not deferred, no open blockers, no
    /// unsatisfied wait-gates.
    Ready,
}

#[derive(Subcommand)]
enum PlanCmd {
    New {
        /// Plan prompt (root node). Optional with --template.
        text: Option<String>,
        /// Template id (in .allternit/rails/templates, .json or .md) or path.
        #[arg(long)]
        template: Option<String>,
        /// Template param `<name>=<value>` (repeatable).
        #[arg(long = "param")]
        params: Vec<String>,
    },
    Refine {
        dag_id: String,
        #[arg(long)]
        delta: String,
        #[arg(long)]
        mutations: Option<PathBuf>,
        #[arg(long)]
        mutations_json: Option<String>,
    },
    Show {
        dag_id: String,
    },
}

#[derive(Subcommand)]
enum NodeCmd {
    Add {
        #[arg(long = "dag")]
        dag_id: String,
        #[arg(long)]
        parent: String,
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "task")]
        kind: String,
        #[arg(long, default_value = "shared")]
        mode: String,
        /// Node prompt; may reference predecessor outputs with
        /// `{{ <node_id>.output }}` / `{{ <node_id>.output_path }}`.
        #[arg(long)]
        description: Option<String>,
        /// `bot:<slug>` | `ao:<harness>` (recorded only).
        #[arg(long)]
        executor: Option<String>,
    },
}

#[derive(Subcommand)]
enum DagCmd {
    Render {
        dag_id: String,
        #[arg(long, default_value = "json")]
        format: String,
    },
}

#[derive(Subcommand)]
enum WihCmd {
    List {
        #[arg(long)]
        ready: bool,
        #[arg(long = "dag")]
        dag_id: Option<String>,
    },
    Pickup {
        node_id: String,
        #[arg(long = "dag")]
        dag_id: String,
        #[arg(long = "agent")]
        agent_id: String,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        fresh: bool,
    },
    Context {
        wih_id: String,
    },
    SignOpen {
        wih_id: String,
        signature: String,
    },
    Close {
        wih_id: String,
        status: String,
        evidence: Vec<String>,
        /// File whose text becomes the node output (immutable blob + receipt,
        /// derived view nodes/<node_id>.out.md). Counts as evidence.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Who is closing: `user:<id>` or `agent:<id>` (bare id = user).
        /// Matters under the judge policy (`close_by: verifier`,
        /// `verify: judge`); unset counts as the worker.
        #[arg(long)]
        actor: Option<String>,
    },
}

#[derive(Subcommand)]
enum LeaseCmd {
    Request {
        wih_id: String,
        agent_id: String,
        paths: Vec<String>,
        /// Lease TTL in seconds. (Was a positional after the variadic paths,
        /// which clap rejects at runtime — every `lease request` panicked.)
        #[arg(long)]
        ttl: Option<i64>,
    },
    /// Grant a requested lease (makes it count for Gate 2 coverage).
    Grant {
        lease_id: String,
        /// Seconds until the grant expires.
        #[arg(long, default_value_t = 3600)]
        ttl: i64,
    },
    Release {
        lease_id: String,
    },
}

#[derive(Subcommand)]
enum LedgerCmd {
    Tail {
        #[arg(default_value_t = 50)]
        n: usize,
    },
    Trace {
        #[arg(long = "node")]
        node_id: Option<String>,
        #[arg(long = "wih")]
        wih_id: Option<String>,
        #[arg(long = "prompt")]
        prompt_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum IndexCmd {
    Rebuild,
}

#[derive(Subcommand)]
enum MailCmd {
    EnsureThread {
        topic: String,
    },
    Send {
        thread_id: String,
        body_ref: String,
        attachments: Vec<String>,
    },
    RequestReview {
        thread_id: String,
        #[arg(long = "wih")]
        wih_id: String,
        #[arg(long = "diff")]
        diff_ref: String,
    },
    Decide {
        thread_id: String,
        #[arg(long)]
        approve: bool,
        #[arg(long)]
        reject: bool,
        #[arg(long = "notes")]
        notes_ref: Option<String>,
    },
    Inbox {
        thread_id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Ack {
        thread_id: String,
        message_id: String,
        #[arg(long = "agent")]
        agent_id: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    Reserve {
        wih_id: String,
        #[arg(long = "agent")]
        agent_id: String,
        paths: Vec<String>,
        #[arg(long)]
        ttl: Option<i64>,
    },
    Release {
        lease_id: String,
    },
    Share {
        thread_id: String,
        asset_ref: String,
        #[arg(long)]
        note: Option<String>,
    },
    Archive {
        thread_id: String,
        path: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Guard {
        action: String,
        #[arg(long)]
        detail: Option<String>,
    },
}

#[derive(Subcommand)]
enum GateCmd {
    Status,
    Check {
        wih_id: String,
        tool: String,
        paths: Vec<String>,
    },
    Rules,
    Verify {
        #[arg(long)]
        json: bool,
    },
    Decision {
        note: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long = "link")]
        links: Vec<String>,
    },
    Mutate {
        #[arg(long)]
        dag_id: String,
        note: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        mutations: Option<PathBuf>,
        #[arg(long)]
        mutations_json: Option<String>,
    },
}

#[derive(Subcommand)]
enum VaultCmd {
    Archive {
        wih_id: String,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RunnerCmd {
    /// Start the Rails Runner as a long-running loop
    Start,
    /// Run the Rails Runner once (process events up to the latest)
    Once,
}

#[derive(Subcommand)]
enum BusCmd {
    Send {
        to: String,
        from: String,
        kind: String,
        payload: String,
        #[arg(long)]
        transport: Option<String>,
        #[arg(long)]
        correlation_id: Option<String>,
    },
    Poll {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Deliver {
        message_id: i64,
    },
    TmuxSend {
        session: String,
        #[arg(long)]
        pane: Option<String>,
        command: String,
        #[arg(long)]
        iteration: Option<String>,
        #[arg(long)]
        correlation_id: Option<String>,
    },
    SocketSend {
        socket: String,
        payload: String,
        #[arg(long)]
        iteration: Option<String>,
        #[arg(long)]
        correlation_id: Option<String>,
    },
    TmuxRunner {
        session: String,
        #[arg(long)]
        pane: Option<String>,
        #[arg(long)]
        watch: bool,
    },
    /// Run a socket transport (UDS). Unix only.
    #[cfg(unix)]
    SocketRunner {
        socket: String,
        #[arg(long)]
        watch: bool,
    },
}

#[derive(Subcommand)]
enum TransportCmd {
    TmuxInspect {
        #[arg(long)]
        session: Option<String>,
    },
    SocketInspect {
        #[arg(long)]
        socket: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match run().await {
        Ok(()) => Ok(()),
        Err(err) => {
            // Structured gate denials: human line + JSON on stderr, exit 2.
            if let Some(gate_err) = GateError::from_anyhow(&err) {
                eprintln!("error: {gate_err}");
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(gate_err).unwrap_or_default()
                );
                std::process::exit(2);
            }
            Err(err)
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.unwrap_or_else(|| std::env::current_dir().unwrap());

    // Ledger and Mail are pure path/config holders (no disk I/O in their
    // constructors), so they stay eager. Everything heavier — the SQLite-backed
    // stores (Leases, Index, Bus) and their dependents (Receipts, Vault, Gate,
    // WorkOps) — is built lazily by `Stores` only when the invoked subcommand
    // actually uses it, so light subcommands (`ticket ready`, `graph insights`,
    // `mail *` reads) never touch SQLite and cannot abort on SQLITE_CANTOPEN.
    let ledger = Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.clone()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }));

    let stores = Stores::new(root.clone(), ledger.clone());

    let transport_root = Arc::new(root.clone());

    let mail = Mail::new(MailOptions {
        root_dir: Some(root.clone()),
        ledger: ledger.clone(),
        actor_id: Some("mail".to_string()),
        actor_type: None,
        mail_index: None,
    });

    match cli.command {
        Commands::Plan(cmd) => match cmd {
            PlanCmd::New {
                text,
                template,
                params,
            } => {
                let gate = stores.gate().await?;
                match template {
                    Some(template_ref) => {
                        let store = TemplateStore::new(&root)?;
                        let template = store.resolve(&template_ref)?;
                        let params = parse_param_args(&params)?;
                        let result =
                            plan_from_template(&gate, &template, &params, text.as_deref(), None)
                                .await?;
                        println!(
                            "prompt_id: {}\ndag_id: {}\nnode_id: {}\ndelta_id: {}\ntemplate: {}",
                            result.prompt_id,
                            result.dag_id,
                            result.root_node_id,
                            result.delta_id,
                            result.template_id
                        );
                        for (step, node) in &result.nodes {
                            println!("step {step} -> {node}");
                        }
                    }
                    None => {
                        if !params.is_empty() {
                            bail!("--param requires --template");
                        }
                        let text = text.context("plan new needs <text> or --template")?;
                        let (prompt_id, dag_id, node_id) = gate.plan_new(&text, None).await?;
                        println!("prompt_id: {prompt_id}\ndag_id: {dag_id}\nnode_id: {node_id}");
                    }
                }
            }
            PlanCmd::Refine {
                dag_id,
                delta,
                mutations,
                mutations_json,
            } => {
                let gate = stores.gate().await?;
                let mutations = load_mutations(mutations, mutations_json)?;
                let delta_id = gate.plan_refine(&dag_id, &delta, "cli", mutations).await?;
                println!("delta_id: {delta_id}");
            }
            PlanCmd::Show { dag_id } => {
                let dag = resolve_dag(&ledger, &root, &dag_id).await?;
                println!("{}", serde_json::to_string_pretty(&dag)?);
            }
        },
        Commands::Node(cmd) => match cmd {
            NodeCmd::Add {
                dag_id,
                parent,
                title,
                kind,
                mode,
                description,
                executor,
            } => {
                let gate = stores.gate().await?;
                let node_id = format!("n_{}", rand::random::<u32>() % 10_000);
                gate.plan_refine(
                    &dag_id,
                    &format!("node add via CLI: {title}"),
                    "cli",
                    vec![DagMutation::CreateNode {
                        node_id: node_id.clone(),
                        node_kind: kind,
                        title,
                        parent_node_id: Some(parent),
                        execution_mode: mode,
                        description,
                        executor,
                    }],
                )
                .await?;
                println!("node_id: {node_id}");
            }
        },
        Commands::Dag(cmd) => match cmd {
            DagCmd::Render { dag_id, format } => {
                let dag = resolve_dag(&ledger, &root, &dag_id).await?;
                if format == "md" {
                    println!("{}", render_dag_markdown(&dag));
                } else {
                    println!("{}", serde_json::to_string_pretty(&dag)?);
                }
            }
        },
        Commands::Wih(cmd) => match cmd {
            WihCmd::List { ready, dag_id } => {
                let events = ledger.query(LedgerQuery::default()).await?;
                let dag_ids = if let Some(dag_id) = dag_id {
                    vec![dag_id]
                } else {
                    collect_dag_ids(&events)
                };
                let active = active_wih_nodes(&events);
                if ready {
                    // Lazy timer resolution happens on readiness checks. Only
                    // build the Gate (SQLite-backed stores) when a dag actually
                    // has an elapsed, unrecorded timer gate.
                    let now = Utc::now();
                    let timer_dags: Vec<&String> = dag_ids
                        .iter()
                        .filter(|dag_id| {
                            project_dag(&events_for_dag(&events, dag_id), dag_id)
                                .nodes
                                .values()
                                .any(|n| n.wait_gates.iter().any(|g| g.timer_elapsed(now)))
                        })
                        .collect();
                    let mut resolved_any = false;
                    if !timer_dags.is_empty() {
                        let gate = stores.gate().await?;
                        for dag_id in timer_dags {
                            resolved_any |=
                                !gate.resolve_elapsed_timer_gates(dag_id).await?.is_empty();
                        }
                    }
                    let events = if resolved_any {
                        ledger.query(LedgerQuery::default()).await?
                    } else {
                        events
                    };
                    for dag_id in dag_ids {
                        let dag_events = events_for_dag(&events, &dag_id);
                        let dag = project_dag(&dag_events, &dag_id);
                        for node_id in ready_nodes(&dag) {
                            if active.contains_key(&node_id) {
                                continue;
                            }
                            if let Some(node) = dag.nodes.get(&node_id) {
                                println!("{dag_id} {node_id} {}", node.title);
                            }
                        }
                    }
                } else {
                    for (node_id, wih_id) in active {
                        println!("{wih_id} {node_id}");
                    }
                }
            }
            WihCmd::Pickup {
                dag_id,
                node_id,
                agent_id,
                role,
                fresh,
            } => {
                let gate = stores.gate().await?;
                let pickup = gate
                    .wih_pickup_detailed(
                        &dag_id,
                        &node_id,
                        &agent_id,
                        WihPickupOptions { role, fresh },
                    )
                    .await?;
                println!("wih_id: {}", pickup.wih_id);
                if let Some(path) = &pickup.context_pack_path {
                    println!("context_pack_path: {path}");
                }
                if let Some(path) = &pickup.resolved_prompt_path {
                    println!("resolved_prompt_path: {path}");
                }
                if let Some(text) = &pickup.resolved_description {
                    println!("--- resolved prompt ---\n{text}");
                }
            }
            WihCmd::Context { wih_id } => {
                let events = ledger.query(LedgerQuery::default()).await?;
                if let Some(wih) = project_wih_from_events(&events, &wih_id) {
                    if let Some(path) = wih.context_pack_path {
                        let contents = std::fs::read_to_string(path)?;
                        println!("{contents}");
                    } else if wih.resolved_prompt_path.is_none() {
                        println!("context pack not found");
                    }
                    if let Some(path) = wih.resolved_prompt_path {
                        let contents = std::fs::read_to_string(&path)?;
                        println!("--- resolved prompt ({path}) ---\n{contents}");
                    }
                } else {
                    println!("wih not found");
                }
            }
            WihCmd::SignOpen { wih_id, signature } => {
                let gate = stores.gate().await?;
                gate.wih_sign_open(&wih_id, &signature).await?;
                println!("signed");
            }
            WihCmd::Close {
                wih_id,
                status,
                evidence,
                output,
                actor,
            } => {
                let gate = stores.gate().await?;
                let output_text = match &output {
                    Some(path) => Some(
                        std::fs::read_to_string(path)
                            .with_context(|| format!("failed to read --output {path:?}"))?,
                    ),
                    None => None,
                };
                let closer = actor.as_deref().map(parse_actor).transpose()?;
                let closed = gate
                    .wih_close_as(&wih_id, &status, &evidence, output_text.as_deref(), closer.as_ref())
                    .await?;
                println!("closed");
                if let Some(receipt_id) = &closed.output_receipt_id {
                    println!("output_receipt: {receipt_id}");
                }
                if let Some(v) = &closed.verdict {
                    println!("node_status: {}", closed.node_status);
                    println!("verdict: {}", serde_json::to_string(v)?);
                }
            }
        },
        Commands::Lease(cmd) => match cmd {
            LeaseCmd::Request {
                wih_id,
                agent_id,
                paths,
                ttl,
            } => {
                let gate = stores.gate().await?;
                let lease_id = gate.lease_request(&wih_id, &agent_id, paths, ttl).await?;
                println!("lease_id: {lease_id}");
            }
            LeaseCmd::Grant { lease_id, ttl } => {
                let lease_store = stores.leases().await?;
                if lease_store.get(&lease_id).await?.is_none() {
                    bail!("no lease {lease_id}");
                }
                let until = (Utc::now() + ChronoDuration::seconds(ttl)).to_rfc3339();
                lease_store.grant(&lease_id, &until).await?;
                println!("granted_until: {until}");
            }
            LeaseCmd::Release { lease_id } => {
                let lease_store = stores.leases().await?;
                lease_store.release(&lease_id).await?;
                println!("released");
            }
        },
        Commands::Ledger(cmd) => match cmd {
            LedgerCmd::Tail { n } => {
                let events = ledger.tail(n).await?;
                for evt in events {
                    println!("{}", serde_json::to_string(&evt)?);
                }
            }
            LedgerCmd::Trace {
                node_id,
                wih_id,
                prompt_id,
            } => {
                if node_id.is_none() && wih_id.is_none() && prompt_id.is_none() {
                    println!("trace requires --node-id, --wih-id, or --prompt-id");
                } else {
                    let events = ledger.query(LedgerQuery::default()).await?;
                    for evt in events.into_iter().filter(|evt| {
                        matches_trace(
                            evt,
                            node_id.as_deref(),
                            wih_id.as_deref(),
                            prompt_id.as_deref(),
                        )
                    }) {
                        println!("{}", serde_json::to_string_pretty(&evt)?);
                    }
                }
            }
        },
        Commands::Index(cmd) => match cmd {
            IndexCmd::Rebuild => {
                let index = stores.index().await?;
                let count = index.rebuild_from_ledger(&ledger).await?;
                println!("indexed {count} events");
            }
        },
        Commands::Mail(cmd) => match cmd {
            MailCmd::EnsureThread { topic } => {
                let thread_id = mail.ensure_thread(&topic).await?;
                println!("thread_id: {thread_id}");
            }
            MailCmd::Send {
                thread_id,
                body_ref,
                attachments,
            } => {
                mail.send_message(&thread_id, &body_ref, attachments.clone())
                    .await?;
                let bus = stores.bus().await?;
                let bus_payload = json!({
                    "thread_id": thread_id,
                    "body_ref": body_ref,
                    "attachments": attachments
                });
                let _ = bus
                    .send_message(NewBusMessage {
                        correlation_id: create_event_id(),
                        to: format!("mail:{}", thread_id),
                        from: "mail".to_string(),
                        kind: "mail_message".to_string(),
                        payload: bus_payload,
                        transport: "mail".to_string(),
                    })
                    .await;
                println!("sent");
            }
            MailCmd::RequestReview {
                thread_id,
                wih_id,
                diff_ref,
            } => {
                mail.request_review(&thread_id, &wih_id, &diff_ref).await?;
                println!("review requested");
            }
            MailCmd::Decide {
                thread_id,
                approve,
                reject,
                notes_ref,
            } => {
                let decision = if approve && !reject {
                    "approve"
                } else if reject && !approve {
                    "reject"
                } else {
                    println!("choose exactly one of --approve or --reject");
                    return Ok(());
                };
                mail.decide_review(&thread_id, decision, notes_ref).await?;
                println!("review decided");
            }
            MailCmd::Inbox { thread_id, limit } => {
                let events = mail.list_messages(thread_id.as_deref(), limit).await?;
                for evt in events {
                    println!("{}", serde_json::to_string_pretty(&evt)?);
                }
            }
            MailCmd::Ack {
                thread_id,
                message_id,
                agent_id,
                note,
            } => {
                let ack_id = mail
                    .acknowledge_message(
                        &thread_id,
                        &message_id,
                        agent_id.as_deref(),
                        note.as_deref(),
                    )
                    .await?;
                println!("ack_id: {ack_id}");
            }
            MailCmd::Reserve {
                wih_id,
                agent_id,
                paths,
                ttl,
            } => {
                let lease_store = stores.leases().await?;
                let lease_id = create_lease_id();
                let req = LeaseRequest {
                    lease_id: lease_id.clone(),
                    wih_id: wih_id.clone(),
                    agent_id: agent_id.clone(),
                    paths: paths.clone(),
                    requested_at: Utc::now().to_rfc3339(),
                    ttl_seconds: ttl,
                };
                lease_store.request(req).await?;
                let _ = mail
                    .log_event(
                        "MailLeaseRequested",
                        json!({
                            "lease_id": lease_id,
                            "wih_id": wih_id,
                            "agent_id": agent_id,
                            "paths": paths,
                            "ttl_seconds": ttl
                        }),
                    )
                    .await?;
                println!("lease_id: {lease_id}");
            }
            MailCmd::Release { lease_id } => {
                let lease_store = stores.leases().await?;
                lease_store.release(&lease_id).await?;
                let _ = mail
                    .log_event("MailLeaseReleased", json!({ "lease_id": lease_id }))
                    .await?;
                println!("released");
            }
            MailCmd::Share {
                thread_id,
                asset_ref,
                note,
            } => {
                let share_id = mail
                    .share_asset(&thread_id, &asset_ref, note.as_deref())
                    .await?;
                println!("share_id: {share_id}");
            }
            MailCmd::Archive {
                thread_id,
                path,
                reason,
            } => {
                let archive_id = mail
                    .archive_thread(&thread_id, &path, reason.as_deref())
                    .await?;
                println!("archive_id: {archive_id}");
            }
            MailCmd::Guard { action, detail } => {
                let guard_id = mail.guard_action(&action, detail.as_deref()).await?;
                println!("guard_id: {guard_id}");
            }
        },
        Commands::Gate(cmd) => match cmd {
            GateCmd::Status => {
                println!("gate: ok");
            }
            GateCmd::Check {
                wih_id,
                tool,
                paths,
            } => {
                let gate = stores.gate().await?;
                let res = gate.pre_tool(&wih_id, &tool, &paths).await?;
                println!("allowed: {}", res.allowed);
                if let Some(reason) = res.reason {
                    println!("reason: {reason}");
                }
            }
            GateCmd::Rules => {
                if let Some(contents) = read_spec("GATE_RULES.md") {
                    println!("{contents}");
                } else {
                    println!("gate rules not found");
                }
            }
            GateCmd::Verify { json } => {
                let events = ledger.query(LedgerQuery::default()).await?;
                let mut ok = true;
                let (chain_ok, chain_issues) = ledger.verify_chain().await?;
                if !chain_ok {
                    ok = false;
                }
                let mut cycle_dags = Vec::new();
                for dag_id in collect_dag_ids(&events) {
                    let dag_events = events_for_dag(&events, &dag_id);
                    let dag = project_dag(&dag_events, &dag_id);
                    if has_cycle_edges(&dag.edges) {
                        ok = false;
                        cycle_dags.push(dag_id);
                    }
                }
                if json {
                    let payload = serde_json::json!({
                        "ok": ok,
                        "ledger_chain_ok": chain_ok,
                        "ledger_chain_issues": chain_issues,
                        "cycle_dags": cycle_dags
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    if !chain_ok {
                        println!("ledger chain failed: {:?}", chain_issues);
                    }
                    for dag_id in &cycle_dags {
                        println!("cycle detected in dag {dag_id}");
                    }
                    if ok {
                        println!("gate verify ok");
                    } else {
                        println!("gate verify failed");
                    }
                }
            }
            GateCmd::Decision {
                note,
                reason,
                links,
            } => {
                let gate = stores.gate().await?;
                let decision_id = gate.record_agent_decision(&note, reason, links).await?;
                println!("decision_id: {decision_id}");
            }
            GateCmd::Mutate {
                dag_id,
                note,
                reason,
                mutations,
                mutations_json,
            } => {
                let gate = stores.gate().await?;
                let mutations = load_mutations(mutations, mutations_json)?;
                let (decision_id, mutation_ids) = gate
                    .mutate_with_decision(&dag_id, &note, reason, mutations)
                    .await?;
                println!("decision_id: {decision_id}");
                for id in mutation_ids {
                    println!("mutation_id: {id}");
                }
            }
        },
        Commands::Vault(cmd) => match cmd {
            VaultCmd::Archive { wih_id } => {
                let vault = stores.vault().await?;
                let path = vault.archive_wih(&wih_id).await?;
                println!("vaulted: {}", path.display());
            }
            VaultCmd::Status { json } => {
                let events = ledger.query(LedgerQuery::default()).await?;
                let mut created: HashMap<String, String> = HashMap::new();
                let mut completed: HashMap<String, String> = HashMap::new();
                for evt in &events {
                    if evt.r#type == "VaultJobCreated" {
                        if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                            created.insert(wih_id.to_string(), evt.ts.clone());
                        }
                    }
                    if evt.r#type == "VaultJobCompleted" {
                        if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                            completed.insert(wih_id.to_string(), evt.ts.clone());
                        }
                    }
                }
                if json {
                    let mut jobs = Vec::new();
                    for (wih_id, created_at) in &created {
                        jobs.push(serde_json::json!({
                            "wih_id": wih_id,
                            "created_at": created_at,
                            "completed_at": completed.get(wih_id),
                            "status": if completed.contains_key(wih_id) { "completed" } else { "pending" }
                        }));
                    }
                    for (wih_id, done_at) in &completed {
                        if !created.contains_key(wih_id) {
                            jobs.push(serde_json::json!({
                                "wih_id": wih_id,
                                "created_at": null,
                                "completed_at": done_at,
                                "status": "completed"
                            }));
                        }
                    }
                    let payload = serde_json::json!({ "jobs": jobs });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    for (wih_id, created_at) in &created {
                        if let Some(done_at) = completed.get(wih_id) {
                            println!("{wih_id} completed {done_at} (created {created_at})");
                        } else {
                            println!("{wih_id} pending (created {created_at})");
                        }
                    }
                    for (wih_id, done_at) in &completed {
                        if !created.contains_key(wih_id) {
                            println!("{wih_id} completed {done_at}");
                        }
                    }
                }
            }
        },
        Commands::Runner(cmd) => {
            let gate = stores.gate().await?;
            let lease_store = stores.leases().await?;
            let receipts = stores.receipts().await?;
            let index = stores.index().await?;
            let vault = stores.vault().await?;
            let bus = stores.bus().await?;
            match cmd {
                RunnerCmd::Start => {
                    start_runner(
                        &root,
                        ledger.clone(),
                        gate,
                        lease_store,
                        receipts,
                        index,
                        vault,
                        bus,
                        true,
                    )
                    .await?;
                }
                RunnerCmd::Once => {
                    start_runner(
                        &root,
                        ledger.clone(),
                        gate,
                        lease_store,
                        receipts,
                        index,
                        vault,
                        bus,
                        false,
                    )
                    .await?;
                }
            }
        }
        Commands::Bus(cmd) => match cmd {
            BusCmd::Send {
                to,
                from,
                kind,
                payload,
                transport,
                correlation_id,
            } => {
                let correlation = correlation_id.unwrap_or_else(|| create_event_id());
                let payload_value: Value =
                    serde_json::from_str(&payload).unwrap_or_else(|_| json!({ "text": payload }));
                let transport = transport.unwrap_or_else(|| "general".to_string());
                let msg = NewBusMessage {
                    correlation_id: correlation,
                    to,
                    from,
                    kind,
                    payload: payload_value,
                    transport,
                };
                let bus = stores.bus().await?;
                let message_id = bus.send_message(msg).await?;
                println!("message_id: {message_id}");
            }
            BusCmd::Poll { limit } => {
                let bus = stores.bus().await?;
                let entries = bus.poll_pending(limit).await?;
                for entry in entries {
                    println!("{}", serde_json::to_string_pretty(&entry)?);
                }
            }
            BusCmd::Deliver { message_id } => {
                let bus = stores.bus().await?;
                bus.mark_delivered(message_id).await?;
                println!("delivered: {message_id}");
            }
            BusCmd::TmuxSend {
                session,
                command,
                pane,
                iteration,
                correlation_id,
            } => {
                let pane_name = pane.clone().unwrap_or_else(|| session.clone());
                let iteration_id = iteration.unwrap_or_else(|| "default".to_string());
                let mut state = load_transport_state(&root, &pane_name, &iteration_id)?;
                if state.busy {
                    bail!(
                        "tmux pane {} (iteration {}) is busy since {:?}",
                        pane_name,
                        iteration_id,
                        state.busy_since
                    );
                }
                state.busy = true;
                state.busy_since = Some(Utc::now().to_rfc3339());
                state.owner = Some("rails".to_string());
                state.iteration_id = Some(iteration_id.clone());
                persist_transport_state(&root, &pane_name, &iteration_id, &state)?;

                let correlation = correlation_id.unwrap_or_else(|| create_event_id());
                let mut payload = json!({
                    "session": session.clone(),
                    "command": command,
                    "context_iteration": iteration_id.clone(),
                });
                payload["pane"] = json!(pane_name.clone());
                let msg = NewBusMessage {
                    correlation_id: correlation,
                    to: format!("tmux:{}", pane_name),
                    from: "rails".to_string(),
                    kind: "tmux_command".to_string(),
                    payload,
                    transport: "tmux".to_string(),
                };
                let bus = stores.bus().await?;
                let id = bus.send_message(msg).await?;
                println!("tmux message_id: {id}");
            }
            BusCmd::SocketSend {
                socket,
                payload,
                iteration,
                correlation_id,
            } => {
                let correlation = correlation_id.unwrap_or_else(|| create_event_id());
                let mut payload_value: Value = serde_json::from_str(&payload)
                    .unwrap_or_else(|_| json!({ "payload": payload }));
                let iteration_id = iteration
                    .clone()
                    .or_else(|| {
                        payload_value
                            .get("context_iteration")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| "default".to_string());
                if let Some(map) = payload_value.as_object_mut() {
                    map.insert("context_iteration".to_string(), json!(iteration_id.clone()));
                }
                let mut state = load_socket_transport_state(&root, &socket, &iteration_id)?;
                if state.busy {
                    bail!(
                        "socket {} (iteration {}) is busy since {:?}",
                        socket,
                        iteration_id,
                        state.busy_since
                    );
                }
                state.busy = true;
                state.busy_since = Some(Utc::now().to_rfc3339());
                state.owner = Some("rails".to_string());
                state.iteration_id = Some(iteration_id.clone());
                persist_socket_transport_state(&root, &socket, &iteration_id, &state)?;
                let msg = NewBusMessage {
                    correlation_id: correlation,
                    to: format!("socket:{}", socket),
                    from: "rails".to_string(),
                    kind: "socket_message".to_string(),
                    payload: payload_value,
                    transport: "socket".to_string(),
                };
                let bus = stores.bus().await?;
                let id = bus.send_message(msg).await?;
                println!("socket message_id: {id}");
            }
            BusCmd::TmuxRunner {
                session,
                pane,
                watch,
            } => {
                let bus = stores.bus().await?;
                let pane_name = pane.unwrap_or_else(|| session.clone());
                run_tmux_transport(transport_root.clone(), bus, pane_name, watch).await?;
            }
            #[cfg(unix)]
            BusCmd::SocketRunner { socket, watch } => {
                let bus = stores.bus().await?;
                run_socket_transport(
                    transport_root.clone(),
                    bus,
                    PathBuf::from(socket),
                    watch,
                )
                .await?;
            }
        },
        Commands::Transport(cmd) => match cmd {
            TransportCmd::TmuxInspect { session } => {
                inspect_tmux_transports(&root, session.as_deref())?;
            }
            TransportCmd::SocketInspect { socket } => {
                inspect_socket_transports(&root, socket.as_deref())?;
            }
        },
        Commands::Init => {
            // `init` explicitly provisions the workspace, so it constructs the
            // full set of stores (including the SQLite-backed ones) on purpose.
            let lease_store = stores.leases().await?;
            let receipts = stores.receipts().await?;
            let index = stores.index().await?;
            init_system(&root, &ledger, &lease_store, &receipts, &index).await?;
        }
        Commands::Judge(cmd) => {
            let ctx = JudgeContext {
                root: root.clone(),
                ledger: ledger.clone(),
                gate: stores.gate().await?,
            };
            run_judge_command(&ctx, cmd).await?
        }
        Commands::Leases(cmd) => {
            let ctx = JudgeContext {
                root: root.clone(),
                ledger: ledger.clone(),
                gate: stores.gate().await?,
            };
            run_leases_command(&ctx, cmd).await?
        }
        Commands::Work(cmd) => {
            let work_ctx = WorkContext {
                root: root.clone(),
                ledger: ledger.clone(),
                gate: stores.gate().await?,
                work: stores.work_ops().await?,
                index: stores.index().await?,
            };
            run_work_command(&work_ctx, cmd).await?
        }
        Commands::Ticket(cmd) => match cmd {
            TicketCmd::Ready => {
                let store = TicketStore::new(&root)?;
                let graph = load_graph(&root)?;
                let gates = WaitGateStore::new(&root)?;
                let all = store.list()?;
                let ready = tickets::ready(&all, &graph, &gates, Utc::now())?;
                println!("{}", serde_json::to_string_pretty(&ready)?);
            }
        },
        // Graph analytics: same view-model builders as the HTTP surface
        // (`graph::views`), so the printed JSON matches /api/rails/graph/*.
        Commands::Graph(cmd) => {
            let store = TicketStore::new(&root)?;
            let graph = load_graph(&root)?;
            let gates = WaitGateStore::new(&root)?;
            let all = store.list()?;
            let ready = tickets::ready(&all, &graph, &gates, Utc::now())?;
            let insights =
                GraphAnalytics::new().compute_insights(&graph, &InsightsConfig::default());
            let view = GraphView::from_graph(&graph);
            match cmd {
                GraphCmd::Insights => {
                    let out = views::build_insights_view(&insights, &ready, &view);
                    println!("{}", serde_json::to_string_pretty(&out)?);
                }
                GraphCmd::Triage => {
                    let out = views::build_triage_view(&insights, &ready, &view);
                    println!("{}", serde_json::to_string_pretty(&out)?);
                }
                GraphCmd::Impact { ticket_id } => {
                    let id: TicketId = ticket_id
                        .parse()
                        .map_err(|e: allternit_commrails::rails_id::InvalidTicketId| {
                            anyhow::anyhow!(e)
                        })?;
                    match views::build_impact_view(&insights, &view, &all, &id) {
                        Ok(out) => println!("{}", serde_json::to_string_pretty(&out)?),
                        Err(e) => bail!("{e}"),
                    }
                }
            }
        }
        Commands::Peer(cmd) => {
            run_peer_command(&root, cmd).await?;
        }
        Commands::Orchestrator(cmd) => {
            run_orchestrator_command(&root, cmd).await?;
        }
        Commands::Steer(cmd) => {
            run_steer_command(&root, cmd).await?;
        }
        Commands::Hook(cmd) => {
            run_hook_command(&root, &stores, &ledger, cmd).await?;
        }
        Commands::WaitGate(cmd) => match cmd {
            WaitGateCmd::Add {
                node,
                kind,
                description,
                until,
                repo,
                run_id,
                pr,
            } => {
                let (dag_id, node_id) = split_node_ref(&node)?;
                let mut params = HashMap::new();
                if let Some(until) = until {
                    params.insert("until".to_string(), json!(until));
                }
                if let Some(repo) = repo {
                    params.insert("repo".to_string(), json!(repo));
                }
                if let Some(run_id) = run_id {
                    params.insert("run_id".to_string(), json!(run_id));
                }
                if let Some(pr) = pr {
                    params.insert("pr".to_string(), json!(pr));
                }
                let gate = stores.gate().await?;
                let gate_id = gate
                    .add_node_wait_gate(&dag_id, &node_id, kind, description, params, "cli")
                    .await?;
                println!("gate_id: {gate_id}");
            }
            WaitGateCmd::Resolve {
                node,
                gate_id,
                outcome,
                actor,
                reason,
            } => {
                let (dag_id, node_id) = split_node_ref(&node)?;
                let resolver = actor.as_deref().map(parse_actor).transpose()?;
                let gate = stores.gate().await?;
                gate.resolve_node_wait_gate(&dag_id, &node_id, &gate_id, outcome, resolver, reason)
                    .await?;
                println!("resolved {gate_id} as {outcome}");
            }
            WaitGateCmd::List { node, dag_id } => {
                let (dag_id, only_node) = match (node, dag_id) {
                    (Some(node), _) => {
                        let (d, n) = split_node_ref(&node)?;
                        (d, Some(n))
                    }
                    (None, Some(d)) => (d, None),
                    (None, None) => bail!("wait-gate list needs --node <dag>/<node> or --dag <dag>"),
                };
                let events = ledger.query(LedgerQuery::default()).await?;
                let dag = project_dag(&events_for_dag(&events, &dag_id), &dag_id);
                let mut nodes: Vec<_> = dag
                    .nodes
                    .values()
                    .filter(|n| only_node.as_deref().is_none_or(|id| id == n.node_id))
                    .collect();
                nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
                let now = Utc::now();
                for n in nodes {
                    for g in &n.wait_gates {
                        let state = match g.outcome {
                            Some(o) => format!(
                                "resolved {o} by {}",
                                g.resolved_by.as_deref().unwrap_or("?")
                            ),
                            None if g.is_satisfied(now) => "elapsed (resolves on next readiness check)".to_string(),
                            None => "open".to_string(),
                        };
                        println!("{} {} [{}] {} - {}", n.node_id, g.gate_id, g.kind, g.description, state);
                    }
                }
            }
            WaitGateCmd::Pending { json: as_json } => {
                let events = ledger.query(LedgerQuery::default()).await?;
                let pending = pending_manual_gates(&events);
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&pending)?);
                } else {
                    for p in pending {
                        println!(
                            "{} {} {} [{}] {}{}",
                            p.dag_id,
                            p.node_id,
                            p.gate_id,
                            p.node_title,
                            p.description,
                            if p.deps_done { " (waiting on you)" } else { " (upstream not done)" }
                        );
                    }
                }
            }
        },
    }

    Ok(())
}

async fn run_hook_command(root: &Path, stores: &Stores, ledger: &Arc<Ledger>, cmd: HookCmd) -> Result<()> {
    use allternit_commrails::hook;
    match cmd {
        HookCmd::ClaudePretool { wih, harness, workspace } => {
            let lease_root = workspace.unwrap_or_else(|| root.to_path_buf());
            // A panic would exit 101, which Claude Code treats as a
            // non-blocking hook error (tool runs). Exit 2 blocks instead.
            std::panic::set_hook(Box::new(|info| {
                eprintln!("Allternit spawn gate: internal error (fail closed): {info}");
                std::process::exit(2);
            }));
            let mut raw = String::new();
            let read = tokio::io::stdin().read_to_string(&mut raw).await;
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let parsed = read
                .map_err(anyhow::Error::from)
                .and_then(|_| serde_json::from_str::<Value>(&raw).map_err(anyhow::Error::from))
                .and_then(|v| hook::HookRequest::from_json(&v));
            let (req, decision) = match parsed {
                Err(err) => (
                    None,
                    hook::Decision {
                        verdict: hook::Verdict::Deny(format!("unreadable hook input (fail closed): {err}")),
                        paths: Vec::new(),
                    },
                ),
                Ok(req) => {
                    let decision = match wih.as_deref() {
                        None => hook::decide(&req, &lease_root, home.as_deref(), None).await,
                        Some(wih_id) => match (stores.gate().await, stores.leases().await) {
                            (Ok(gate), Ok(leases)) => {
                                hook::decide(
                                    &req,
                                    &lease_root,
                                    home.as_deref(),
                                    Some(hook::WihBinding {
                                        wih_id,
                                        gate: &gate,
                                        leases: &leases,
                                    }),
                                )
                                .await
                            }
                            (Err(err), _) | (_, Err(err)) => hook::Decision {
                                verdict: hook::Verdict::Deny(format!(
                                    "Gate 2 unavailable for WIH {wih_id} (fail closed): {err}"
                                )),
                                paths: Vec::new(),
                            },
                        },
                    };
                    (Some(req), decision)
                }
            };
            // Every WIH-bound decision and every denial is recorded. A ledger
            // failure never flips a deny into an allow.
            if wih.is_some() || decision.verdict.is_deny() {
                let req_for_event = req.unwrap_or(hook::HookRequest {
                    tool_name: "<unparsed>".to_string(),
                    tool_input: Value::Null,
                    cwd: None,
                    session_id: None,
                });
                let event = hook::decision_event(&req_for_event, &harness, wih.as_deref(), &decision);
                if let Err(err) = ledger.append(event).await {
                    eprintln!("allternit spawn gate: ledger append failed: {err}");
                }
            }
            if let Some(out) = hook::claude_hook_output(&decision.verdict) {
                println!("{out}");
            }
        }
        HookCmd::ClaudeSettings { wih, out, workspace } => {
            let bin = std::env::current_exe().context("locating allternit-commrails")?;
            let settings = hook::claude_settings(hook::HookTarget {
                commrails_bin: &bin,
                root,
                workspace: workspace.as_deref(),
                wih_id: wih.as_deref(),
            });
            let text = serde_json::to_string_pretty(&settings)?;
            match out {
                Some(path) => {
                    fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
                    println!("{}", path.display());
                }
                None => println!("{text}"),
            }
        }
        HookCmd::SpawnCheck { harness, wih } => {
            let policy = match wih.as_deref() {
                Some(wih_id) => Some(hook::load_wih_policy(ledger, wih_id).await?),
                None => None,
            };
            match hook::admit(&harness, policy.as_ref()) {
                Ok(gate) => println!("admitted: {harness} ({})", gate.as_str()),
                Err(reason) => {
                    if let Some(wih_id) = wih.as_deref() {
                        let _ = ledger.append(hook::spawn_refused_event(&harness, wih_id, &reason)).await;
                    }
                    eprintln!("{reason}");
                    std::process::exit(3);
                }
            }
        }
    }
    Ok(())
}

async fn run_steer_command(root: &Path, cmd: SteerCmd) -> Result<()> {
    let ledger = Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.to_path_buf()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }));
    let steer = Steer::new(ledger);

    match cmd {
        SteerCmd::Checkpoint { cwd } => {
            let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| root.to_path_buf()));
            let result = steer.checkpoint(&cwd).await?;
            println!("hash: {}", result.hash);
            println!("changed: {}", result.changed);
            if let Some(id) = result.event_id {
                println!("event_id: {}", id);
            }
        }
        SteerCmd::Consult { cwd, prompt_file } => {
            let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| root.to_path_buf()));
            let context = if let Some(path) = prompt_file {
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading prompt file {}", path.display()))?
            } else {
                steer.build_context(&cwd)?
            };
            let answer = steer.consult(&cwd, &context).await?;
            println!("{}", answer);
        }
        SteerCmd::CommitGate { cwd } => {
            let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| root.to_path_buf()));
            let result = steer.commit_gate(&cwd).await?;
            println!("verdict: {}", result.verdict);
            println!("{}", result.body);
            if result.verdict != "APPROVE" {
                std::process::exit(2);
            }
        }
    }
    Ok(())
}

async fn run_peer_command(root: &Path, cmd: PeerCmd) -> Result<()> {
    let registry = PeerRegistry::new(root)?;
    match cmd {
        PeerCmd::Register { name, vendor, cwd } => {
            let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap());
            let peer = registry.register(&name, cwd, &vendor)?;
            println!("peer_id: {}", peer.peer_id);
            println!("name: {}", peer.name);
            println!("inbox: {}", peer.inbox_socket.display());
        }
        PeerCmd::Unregister { id_or_name } => {
            if registry.unregister(&id_or_name)? {
                println!("unregistered");
            } else {
                eprintln!("peer not found: {}", id_or_name);
                std::process::exit(1);
            }
        }
        PeerCmd::List { json } => {
            let peers = registry.list();
            if json {
                println!("{}", serde_json::to_string_pretty(&peers)?);
            } else {
                if peers.is_empty() {
                    println!("no peers");
                }
                for p in peers {
                    println!(
                        "{}  {}  {}  {}  {:?}  {}",
                        p.peer_id,
                        p.name,
                        p.vendor,
                        p.cwd.display(),
                        p.status,
                        p.inbox_socket.display()
                    );
                }
            }
        }
        PeerCmd::Heartbeat { id_or_name } => {
            registry.heartbeat(&id_or_name)?;
            println!("ok");
        }
        #[cfg(unix)]
        PeerCmd::Inbox { id_or_name } => {
            let peer = registry
                .resolve(&id_or_name)
                .context(format!("peer not found: {}", id_or_name))?;
            let mut socket = PeerSocket::bind(&peer.inbox_socket).await?;
            println!("inbox_bound: {}", socket.socket_path().display());
            loop {
                let (envelope, _) = socket.accept_envelope().await?;
                println!("{}", serde_json::to_string_pretty(&envelope)?);
            }
        }
        PeerCmd::Send { to, message } => {
            let peer = registry
                .resolve(&to)
                .context(format!("peer not found: {}", to))?;
            let envelope = PeerEnvelope::new("cli", &peer.name, message);
            let receipt = send_envelope(
                &peer.inbox_socket,
                &envelope,
                TokioDuration::from_secs(5),
            )
            .await?;
            if receipt.delivered {
                println!("delivered");
            } else {
                bail!(
                    "{}",
                    receipt.error.unwrap_or_else(|| "unknown delivery failure".to_string())
                );
            }
        }
    }
    Ok(())
}

async fn run_orchestrator_command(root: &Path, cmd: OrchestratorCmd) -> Result<()> {
    let orch = Orchestrator::new(root.to_path_buf())?;
    match cmd {
        OrchestratorCmd::Spawn {
            slug,
            repo,
            cmd,
            worktree,
            vendor,
            mode,
            task_file,
            notes_sentinel,
            wih,
        } => {
            let result = orch
                .spawn(SpawnOptions {
                    slug: &slug,
                    repo: &repo,
                    cmd: &cmd,
                    worktree,
                    vendor: &vendor,
                    mode: &mode,
                    task_file: task_file.as_deref(),
                    notes_sentinel: notes_sentinel.as_deref(),
                    wih: wih.as_deref(),
                })
                .await?;
            println!(
                "{} {} {} {} {}",
                result.session,
                result.workdir.display(),
                result.logfile.display(),
                result.peer_id,
                result.inbox_socket.display()
            );
        }
        OrchestratorCmd::Send { slug, data } => {
            let text = data.join(" ");
            orch.send(&slug, &text).await?;
            println!("submitted to ao-{}", slug);
        }
        OrchestratorCmd::Watch {
            slug,
            sentinel,
            timeout_seconds,
            interval_seconds,
        } => {
            match orch.watch(&slug, &sentinel, timeout_seconds, interval_seconds).await? {
                WatchOutcome::Done => {
                    println!("DONE {}", sentinel.display());
                }
                WatchOutcome::Dead => {
                    println!("PANE-DEAD ao-{} (agent exited or session gone)", slug);
                    std::process::exit(3);
                }
                WatchOutcome::Timeout => {
                    println!(
                        "TIMEOUT after {}s; sentinel absent, pane alive",
                        timeout_seconds
                    );
                    std::process::exit(4);
                }
            }
        }
        OrchestratorCmd::Status { slug, lines } => {
            let output = orch.status(slug.as_deref(), lines).await?;
            println!("{}", output);
        }
        OrchestratorCmd::Kill { slug, rm_worktree } => {
            orch.kill(&slug, rm_worktree).await?;
            println!("killed ao-{}", slug);
        }
        OrchestratorCmd::Doctor => {
            orch.doctor().await?;
            println!("orchestrator doctor: OK");
        }
    }
    Ok(())
}

/// Lazy store registry for the CLI. Each accessor builds its store on first
/// use (per process) and caches it, so a subcommand only pays for — and only
/// risks failure from — the stores it actually touches. SQLite-backed stores
/// (Leases, Index, Bus) and their dependents (Gate, Vault, WorkOps) live here;
/// Ledger and Mail are pure config holders and stay eagerly built in `main`.
struct Stores {
    root: PathBuf,
    ledger: Arc<Ledger>,
    leases: OnceCell<Arc<Leases>>,
    receipts: OnceCell<Arc<ReceiptStore>>,
    index: OnceCell<Arc<Index>>,
    vault: OnceCell<Arc<Vault>>,
    gate: OnceCell<Arc<Gate>>,
    bus: OnceCell<Arc<Bus>>,
    work_ops: OnceCell<Arc<WorkOps>>,
}

impl Stores {
    fn new(root: PathBuf, ledger: Arc<Ledger>) -> Self {
        Self {
            root,
            ledger,
            leases: OnceCell::new(),
            receipts: OnceCell::new(),
            index: OnceCell::new(),
            vault: OnceCell::new(),
            gate: OnceCell::new(),
            bus: OnceCell::new(),
            work_ops: OnceCell::new(),
        }
    }

    async fn leases(&self) -> Result<Arc<Leases>> {
        Ok(self
            .leases
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(Arc::new(
                    Leases::new(LeasesOptions {
                        root_dir: Some(self.root.clone()),
                        leases_dir: Some(PathBuf::from(".allternit/leases")),
                        event_sink: Some(self.ledger.clone()),
                        actor_id: Some("gate".to_string()),
                        auto_renewal_enabled: true,
                        auto_renewal_threshold_seconds: 300,
                        auto_renewal_interval_seconds: 60,
                        auto_renewal_extend_seconds: 600,
                    })
                    .await?,
                ))
            })
            .await?
            .clone())
    }

    async fn receipts(&self) -> Result<Arc<ReceiptStore>> {
        Ok(self
            .receipts
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(Arc::new(ReceiptStore::new(ReceiptStoreOptions {
                    root_dir: Some(self.root.clone()),
                    receipts_dir: Some(PathBuf::from(".allternit/receipts")),
                    blobs_dir: Some(PathBuf::from(".allternit/blobs")),
                })?))
            })
            .await?
            .clone())
    }

    async fn index(&self) -> Result<Arc<Index>> {
        Ok(self
            .index
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(Arc::new(
                    Index::new(IndexOptions {
                        root_dir: Some(self.root.clone()),
                        index_dir: Some(PathBuf::from(".allternit/index")),
                    })
                    .await?,
                ))
            })
            .await?
            .clone())
    }

    async fn vault(&self) -> Result<Arc<Vault>> {
        Ok(self
            .vault
            .get_or_init(|| async {
                Arc::new(Vault::new(VaultOptions {
                    root_dir: Some(self.root.clone()),
                    ledger: self.ledger.clone(),
                    actor_id: Some("gate".to_string()),
                }))
            })
            .await
            .clone())
    }

    async fn gate(&self) -> Result<Arc<Gate>> {
        Ok(self
            .gate
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(Arc::new(Gate::new(GateOptions {
                    ledger: self.ledger.clone(),
                    leases: self.leases().await?,
                    receipts: self.receipts().await?,
                    index: Some(self.index().await?),
                    vault: Some(self.vault().await?),
                    root_dir: Some(self.root.clone()),
                    actor_id: Some("gate".to_string()),
                    oauth_vault: None,
                    strict_provenance: None,
                    visual_provider: None,
                    visual_config: None,
                })))
            })
            .await?
            .clone())
    }

    async fn bus(&self) -> Result<Arc<Bus>> {
        Ok(self
            .bus
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(Arc::new(
                    Bus::new(BusOptions {
                        root_dir: self.root.clone(),
                        ledger: self.ledger.clone(),
                        actor_id: Some("bus".to_string()),
                        actor_type: Some(ActorType::Gate),
                    })
                    .await?,
                ))
            })
            .await?
            .clone())
    }

    async fn work_ops(&self) -> Result<Arc<WorkOps>> {
        Ok(self
            .work_ops
            .get_or_init(|| async {
                Arc::new(WorkOps::new(
                    self.ledger.clone(),
                    Some("allternit work".to_string()),
                    Some(ActorType::Agent),
                ))
            })
            .await
            .clone())
    }
}

fn read_spec(name: &str) -> Option<String> {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = crate_root.join("spec").join(name);
    std::fs::read_to_string(path).ok()
}

async fn init_system(
    root: &PathBuf,
    _ledger: &Ledger,
    _leases: &Leases,
    _receipts: &ReceiptStore,
    _index: &Index,
) -> Result<()> {
    let dirs = [
        ".allternit/ledger/events",
        ".allternit/leases",
        ".allternit/receipts",
        ".allternit/blobs",
        ".allternit/index",
        ".allternit/mail/threads",
        ".allternit/vault",
        ".allternit/work/dags",
        ".allternit/wih",
        ".allternit/transports/tmux",
        ".allternit/transports/socket",
    ];
    for rel in dirs {
        ensure_dir(&root.join(rel))?;
    }

    println!("Initialized workspace stores:");
    println!(
        "  - Ledger JSONL events: {}",
        root.join(".allternit/ledger/events").display()
    );
    println!(
        "  - Leases SQLite: {}",
        root.join(".allternit/leases/leases.db").display()
    );
    println!(
        "  - Receipt store: {}",
        root.join(".allternit/receipts").display()
    );
    println!("  - Blob store: {}", root.join(".allternit/blobs").display());
    println!("  - Index DB: {}", root.join(".allternit/index").display());
    println!(
        "  - Mail threads: {}",
        root.join(".allternit/mail/threads").display()
    );
    println!("  - Vault root: {}", root.join(".allternit/vault").display());
    println!(
        "Ledger, leases, and receipts will automatically log events when you run the CLI commands."
    );
    Ok(())
}

#[derive(Deserialize, Serialize, Default)]
struct TransportState {
    last_message_id: Option<i64>,
    last_output: String,
    busy: bool,
    owner: Option<String>,
    busy_since: Option<String>,
    iteration_id: Option<String>,
}

async fn run_tmux_transport(
    root: Arc<PathBuf>,
    bus: Arc<Bus>,
    pane_name: String,
    watch: bool,
) -> Result<()> {
    let recipient = format!("tmux:{}", pane_name);
    loop {
        let pending = bus.poll_pending_for(&recipient, Some("tmux"), 5).await?;
        if pending.is_empty() {
            if !watch {
                break;
            }
            sleep(TokioDuration::from_secs(2)).await;
            continue;
        }

        for msg in pending {
            if let Some(command) = msg.payload.get("command").and_then(|v| v.as_str()) {
                let iteration_id = msg
                    .payload
                    .get("context_iteration")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                ensure_tmux_pane(&pane_name).await?;
                let root_path = root.as_ref();
                let mut state = load_transport_state(root_path, &pane_name, &iteration_id)?;
                let (status, output) = execute_tmux_command(&pane_name, command)
                    .await
                    .unwrap_or((None, "<tmux execution failed>".to_string()));
                let delta = output
                    .strip_prefix(&state.last_output)
                    .unwrap_or(&output)
                    .to_string();
                state.last_output = output.clone();
                state.busy = false;
                state.iteration_id = Some(iteration_id.clone());
                state.last_message_id = Some(msg.id);
                state.busy_since = None;
                state.owner = Some(msg.from.clone());
                persist_transport_state(root_path, &pane_name, &iteration_id, &state)?;
                let response = NewBusMessage {
                    correlation_id: msg.correlation_id.clone(),
                    to: msg.from.clone(),
                    from: "tmux-transport".to_string(),
                    kind: "tmux_response".to_string(),
                    payload: json!({
                        "session": pane_name.clone(),
                        "command": command,
                        "status": status,
                        "output": output,
                        "delta": delta,
                        "context_iteration": iteration_id,
                    }),
                    transport: "tmux".to_string(),
                };
                let _ = bus.send_message(response).await?;
            }
            bus.mark_delivered(msg.id).await?;
        }

        if !watch {
            break;
        }
    }
    Ok(())
}

async fn execute_tmux_command(session: &str, command: &str) -> Result<(Option<i32>, String)> {
    let status = Command::new("tmux")
        .arg("send-keys")
        .arg("-t")
        .arg(session)
        .arg(command)
        .arg("Enter")
        .status()
        .await?;

    let output = Command::new("tmux")
        .arg("capture-pane")
        .arg("-pt")
        .arg(session)
        .arg("-J")
        .output()
        .await?;

    let result = String::from_utf8_lossy(&output.stdout).to_string();
    Ok((status.code(), result))
}

async fn ensure_tmux_pane(pane: &str) -> Result<()> {
    let output = Command::new("tmux")
        .arg("list-windows")
        .arg("-F")
        .arg("#{window_name}")
        .output()
        .await?;
    let windows = String::from_utf8_lossy(&output.stdout);
    if !windows.lines().any(|line| line == pane) {
        Command::new("tmux")
            .arg("new-window")
            .arg("-n")
            .arg(pane)
            .status()
            .await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn run_socket_transport(
    root: Arc<PathBuf>,
    bus: Arc<Bus>,
    socket: PathBuf,
    watch: bool,
) -> Result<()> {
    let recipient = format!("socket:{}", socket.display());
    let root_path = root.as_ref();
    let socket_dir = root_path
        .join(".allternit")
        .join("transports")
        .join("socket")
        .join(socket.display().to_string());
    ensure_dir(&socket_dir)?;
    loop {
        let pending = bus.poll_pending_for(&recipient, Some("socket"), 5).await?;
        if pending.is_empty() {
            if !watch {
                break;
            }
            sleep(TokioDuration::from_secs(2)).await;
            continue;
        }

        for msg in pending {
            let iteration_id = msg
                .payload
                .get("context_iteration")
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string();
            let socket_name = socket.display().to_string();
            let mut stream = UnixStream::connect(&socket).await?;
            let payload_text = serde_json::to_string(&msg.payload)?;
            stream.write_all(payload_text.as_bytes()).await?;
            stream.write_all(b"\n").await?;

            let mut reader = BufReader::new(stream);
            let mut response_text = String::new();
            reader.read_to_string(&mut response_text).await?;

            let response_payload: Value = serde_json::from_str(&response_text)
                .unwrap_or_else(|_| json!({ "raw": response_text }));
            let status = response_payload
                .get("status")
                .cloned()
                .unwrap_or(Value::Null);
            let stdout = response_payload
                .get("stdout")
                .cloned()
                .unwrap_or(Value::Null);
            let stderr = response_payload
                .get("stderr")
                .cloned()
                .unwrap_or(Value::Null);
            let response = NewBusMessage {
                correlation_id: msg.correlation_id.clone(),
                to: msg.from.clone(),
                from: "socket-transport".to_string(),
                kind: "socket_response".to_string(),
                payload: json!({
                    "socket": socket_name.clone(),
                    "context_iteration": iteration_id.clone(),
                    "status": status,
                    "stdout": stdout,
                    "stderr": stderr,
                    "response": response_payload
                }),
                transport: "socket".to_string(),
            };
            let _ = bus.send_message(response).await?;
            let mut state = load_socket_transport_state(root_path, &socket_name, &iteration_id)?;
            state.last_output = response_text.clone();
            state.busy = false;
            state.busy_since = None;
            state.owner = Some(msg.from.clone());
            state.iteration_id = Some(iteration_id.clone());
            state.last_message_id = Some(msg.id);
            persist_socket_transport_state(root_path, &socket_name, &iteration_id, &state)?;
            bus.mark_delivered(msg.id).await?;
        }

        if !watch {
            break;
        }
    }
    Ok(())
}

fn transport_state_dir(root: &Path, pane: &str, iteration: &str) -> PathBuf {
    root.join(".allternit")
        .join("transports")
        .join("tmux")
        .join(pane)
        .join(iteration)
}

fn load_transport_state(root: &Path, pane: &str, iteration: &str) -> Result<TransportState> {
    let dir = transport_state_dir(root, pane, iteration);
    if let Ok(raw) = std::fs::read_to_string(dir.join("state.json")) {
        serde_json::from_str(&raw).map_err(|err| err.into())
    } else {
        Ok(TransportState::default())
    }
}

fn persist_transport_state(
    root: &Path,
    pane: &str,
    iteration: &str,
    state: &TransportState,
) -> Result<()> {
    let dir = transport_state_dir(root, pane, iteration);
    ensure_dir(&dir)?;
    let path = dir.join("state.json");
    write_json_atomic(&path, state)?;
    Ok(())
}

fn socket_transport_state_dir(root: &Path, socket: &str, iteration: &str) -> PathBuf {
    root.join(".allternit")
        .join("transports")
        .join("socket")
        .join(socket)
        .join(iteration)
}

fn load_socket_transport_state(
    root: &Path,
    socket: &str,
    iteration: &str,
) -> Result<TransportState> {
    let dir = socket_transport_state_dir(root, socket, iteration);
    if let Ok(raw) = std::fs::read_to_string(dir.join("state.json")) {
        serde_json::from_str(&raw).map_err(|err| err.into())
    } else {
        Ok(TransportState::default())
    }
}

fn persist_socket_transport_state(
    root: &Path,
    socket: &str,
    iteration: &str,
    state: &TransportState,
) -> Result<()> {
    let dir = socket_transport_state_dir(root, socket, iteration);
    ensure_dir(&dir)?;
    let path = dir.join("state.json");
    write_json_atomic(&path, state)?;
    Ok(())
}

fn inspect_tmux_transports(root: &Path, session: Option<&str>) -> Result<()> {
    let base = root.join(".allternit").join("transports").join("tmux");
    if !base.exists() {
        println!("No tmux transport contexts found.");
        return Ok(());
    }
    for pane_entry in fs::read_dir(&base)? {
        let pane_entry = pane_entry?;
        if !pane_entry.file_type()?.is_dir() {
            continue;
        }
        let pane_name = pane_entry.file_name().to_string_lossy().to_string();
        if let Some(filter) = session {
            if filter != pane_name {
                continue;
            }
        }
        println!("Pane: {}", pane_name);
        for iteration_entry in fs::read_dir(pane_entry.path())? {
            let iteration_entry = iteration_entry?;
            if !iteration_entry.file_type()?.is_dir() {
                continue;
            }
            let iteration_name = iteration_entry.file_name().to_string_lossy().to_string();
            match load_transport_state(root, &pane_name, &iteration_name) {
                Ok(state) => {
                    print_state_summary("  ", &iteration_name, &state);
                }
                Err(err) => {
                    println!(
                        "  Iteration: {} (failed to read state: {})",
                        iteration_name, err
                    );
                }
            }
        }
    }
    Ok(())
}

fn inspect_socket_transports(root: &Path, socket_filter: Option<&str>) -> Result<()> {
    let base = root.join(".allternit").join("transports").join("socket");
    if !base.exists() {
        println!("No socket transport contexts found.");
        return Ok(());
    }
    for socket_entry in fs::read_dir(&base)? {
        let socket_entry = socket_entry?;
        if !socket_entry.file_type()?.is_dir() {
            continue;
        }
        let socket_name = socket_entry.file_name().to_string_lossy().to_string();
        if let Some(filter) = socket_filter {
            if filter != socket_name {
                continue;
            }
        }
        println!("Socket: {}", socket_name);
        for iteration_entry in fs::read_dir(socket_entry.path())? {
            let iteration_entry = iteration_entry?;
            if !iteration_entry.file_type()?.is_dir() {
                continue;
            }
            let iteration_name = iteration_entry.file_name().to_string_lossy().to_string();
            match load_socket_transport_state(root, &socket_name, &iteration_name) {
                Ok(state) => {
                    print_state_summary("  ", &iteration_name, &state);
                }
                Err(err) => {
                    println!(
                        "  Iteration: {} (failed to read state: {})",
                        iteration_name, err
                    );
                }
            }
        }
    }
    Ok(())
}

fn print_state_summary(prefix: &str, iteration: &str, state: &TransportState) {
    println!("{prefix}Iteration: {iteration}");
    println!(
        "{prefix}  busy: {} since {:?}",
        state.busy, state.busy_since
    );
    println!("{prefix}  owner: {:?}", state.owner);
    println!("{prefix}  last_message_id: {:?}", state.last_message_id);
    println!(
        "{prefix}  last_output: {}",
        preview_output(&state.last_output, 80)
    );
}

fn preview_output(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        text.to_string()
    } else {
        format!("{}…", &text[..limit])
    }
}

async fn start_runner(
    root: &PathBuf,
    ledger: Arc<Ledger>,
    gate: Arc<Gate>,
    lease_store: Arc<Leases>,
    _receipts: Arc<ReceiptStore>,
    index: Arc<Index>,
    vault: Arc<Vault>,
    bus: Arc<Bus>,
    persistent: bool,
) -> Result<()> {
    let meta_dir = root.join(".allternit/meta");
    ensure_dir(&meta_dir)?;
    let state_path = meta_dir.join("rails_runner_state.json");
    let mut state = RunnerState::load(&state_path)?;

    loop {
        process_runner_events(
            root,
            &ledger,
            &vault,
            &lease_store,
            &index,
            &gate,
            &bus,
            &mut state,
            &state_path,
        )
        .await?;

        if !persistent {
            break;
        }
        sleep(TokioDuration::from_secs(5)).await;
    }
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct RunnerState {
    #[serde(default)]
    processed: HashSet<String>,
    cursor: Option<String>,
    #[serde(default)]
    loop_progress: HashMap<String, LoopProgress>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct LoopProgress {
    last_iteration: u32,
    in_progress: bool,
    last_started_event_id: Option<String>,
    last_started_at: Option<String>,
    last_completed_event_id: Option<String>,
    last_completed_at: Option<String>,
    last_outcome: Option<String>,
    escalated: bool,
    spawn_requests: Vec<u32>,
    last_spawned_at: Option<String>,
}

impl RunnerState {
    fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            Ok(serde_json::from_str(&raw).unwrap_or_default())
        } else {
            Ok(RunnerState::default())
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        write_json_atomic(path, self)?;
        Ok(())
    }
}

async fn process_runner_events(
    root: &Path,
    ledger: &Ledger,
    vault: &Vault,
    leases: &Leases,
    index: &Index,
    gate: &Gate,
    bus: &Bus,
    state: &mut RunnerState,
    state_path: &Path,
) -> Result<()> {
    let events = ledger.query(LedgerQuery::default()).await?;
    for evt in &events {
        if state.processed.contains(&evt.event_id) {
            continue;
        }
        if evt.r#type == "WIHClosedSigned" {
            if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                handle_wih_closed(vault, leases, index, ledger, wih_id).await?;
            }
        }
        state.processed.insert(evt.event_id.clone());
        state.cursor = Some(evt.event_id.clone());
        state.save(state_path)?;
    }
    process_loop_iterations(root, ledger, gate, &events, state, state_path).await?;
    process_bus_queue(ledger, bus).await?;
    Ok(())
}

async fn handle_wih_closed(
    vault: &Vault,
    leases: &Leases,
    index: &Index,
    ledger: &Ledger,
    wih_id: &str,
) -> Result<()> {
    println!("Autopipeline: vaulting {}", wih_id);
    let start_event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: None,
        r#type: "VaultArchiveStarted".to_string(),
        payload: json!({ "wih_id": wih_id }),
        provenance: None,
    };
    ledger.append(start_event).await?;
    let path = vault.archive_wih(wih_id).await?;
    println!("Autopipeline: vaulted {} -> {}", wih_id, path.display());
    let completed_event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: None,
        r#type: "VaultArchiveCompleted".to_string(),
        payload: json!({ "wih_id": wih_id, "path": path.to_string_lossy() }),
        provenance: None,
    };
    ledger.append(completed_event).await?;
    let released = leases.release_for_wih(wih_id).await?;
    if !released.is_empty() {
        println!("Autopipeline: released leases {:?}", released);
    }
    let count = index.rebuild_from_ledger(ledger).await?;
    println!("Autopipeline: index rebuild touched {count} events");
    Ok(())
}

async fn process_loop_iterations(
    root: &Path,
    ledger: &Ledger,
    gate: &Gate,
    events: &[AllternitEvent],
    state: &mut RunnerState,
    state_path: &Path,
) -> Result<()> {
    let active_wihs = collect_active_wih_ids(events);
    for wih_id in active_wihs {
        if is_wih_closed(events, &wih_id) {
            state.loop_progress.remove(&wih_id);
            continue;
        }
        let policy = loop_policy_from_events(events, &wih_id);
        let progress = state.loop_progress.entry(wih_id.clone()).or_default();
        if progress.escalated {
            continue;
        }

        if progress.in_progress {
            if let Some(started_at) = &progress.last_started_at {
                let receipts = collect_receipts_after(events, &wih_id, started_at);
                if evidence_satisfied(receipts.len(), &policy)
                    && acceptance_refs_satisfied(events, &wih_id, &policy.acceptance_refs)
                {
                    let completed_event_id = emit_loop_completed(
                        ledger,
                        &wih_id,
                        progress.last_iteration,
                        "accepted",
                        receipts.len(),
                        !policy.acceptance_refs.is_empty(),
                    )
                    .await?;
                    progress.in_progress = false;
                    progress.last_completed_event_id = Some(completed_event_id);
                    progress.last_completed_at = Some(Utc::now().to_rfc3339());
                    progress.last_outcome = Some("accepted".to_string());
                    gate.turn_closeout(&wih_id).await?;
                    state.save(state_path)?;
                }
            }
            continue;
        }

        if progress.last_iteration >= policy.max_iterations {
            if policy.escalate_on_max_iterations && !progress.escalated {
                let _ = emit_loop_escalated(
                    ledger,
                    &wih_id,
                    progress.last_iteration,
                    "max_iterations reached",
                )
                .await?;
                progress.escalated = true;
                progress.last_outcome = Some("max_iterations reached".to_string());
                state.save(state_path)?;
            }
            continue;
        }

        if should_spawn_subagent(&progress, &policy) {
            let next_iter = progress.last_iteration + 1;
            ensure_loop_policy_injected(root, ledger, &wih_id, next_iter).await?;
            let _spawn_event = emit_loop_spawn_requested(ledger, &wih_id, next_iter).await?;
            let event_id = emit_loop_started(ledger, &wih_id, next_iter, &policy).await?;
            progress.spawn_requests.push(next_iter);
            progress.last_spawned_at = Some(Utc::now().to_rfc3339());
            progress.last_iteration = next_iter;
            progress.in_progress = true;
            progress.last_started_event_id = Some(event_id.clone());
            progress.last_started_at = Some(Utc::now().to_rfc3339());
            progress.last_completed_event_id = None;
            progress.last_completed_at = None;
            progress.last_outcome = Some("spawn_requested".to_string());
            state.save(state_path)?;
            continue;
        }

        let next_iter = progress.last_iteration + 1;
        ensure_loop_policy_injected(root, ledger, &wih_id, next_iter).await?;
        let event_id = emit_loop_started(ledger, &wih_id, next_iter, &policy).await?;
        progress.last_iteration = next_iter;
        progress.in_progress = true;
        progress.last_started_event_id = Some(event_id.clone());
        progress.last_started_at = Some(Utc::now().to_rfc3339());
        progress.last_completed_event_id = None;
        progress.last_completed_at = None;
        progress.last_outcome = Some("iteration_started".to_string());
        state.save(state_path)?;
    }
    Ok(())
}

fn collect_active_wih_ids(events: &[AllternitEvent]) -> Vec<String> {
    let mut created = HashSet::new();
    let mut closed = HashSet::new();
    for evt in events {
        if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
            match evt.r#type.as_str() {
                "WIHCreated" => {
                    created.insert(wih_id.to_string());
                }
                "WIHClosedSigned" | "WIHArchived" => {
                    closed.insert(wih_id.to_string());
                }
                _ => {}
            }
        }
    }
    created
        .into_iter()
        .filter(|id| !closed.contains(id))
        .collect()
}

fn is_wih_closed(events: &[AllternitEvent], wih_id: &str) -> bool {
    events.iter().any(|evt| {
        evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id)
            && (evt.r#type == "WIHClosedSigned" || evt.r#type == "WIHArchived")
    })
}

fn loop_policy_from_events(events: &[AllternitEvent], wih_id: &str) -> LoopPolicy {
    for evt in events.iter().rev() {
        if evt.r#type != "WIHCreated" {
            continue;
        }
        if evt.payload.get("wih_id").and_then(|v| v.as_str()) != Some(wih_id) {
            continue;
        }
        if let Some(policy_value) = evt.payload.get("policy").and_then(|p| p.get("loop_policy")) {
            if let Ok(policy) = serde_json::from_value::<LoopPolicy>(policy_value.clone()) {
                return policy;
            }
        }
    }
    LoopPolicy::default()
}

async fn emit_loop_started(
    ledger: &Ledger,
    wih_id: &str,
    iteration: u32,
    policy: &LoopPolicy,
) -> Result<String> {
    let payload = json!({
        "wih_id": wih_id,
        "iteration": iteration,
        "started_at": Utc::now().to_rfc3339(),
        "policy": serde_json::to_value(policy).unwrap_or_else(|_| json!({}))
    });
    let event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: Some(EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        }),
        r#type: "RailsLoopIterationStarted".to_string(),
        payload,
        provenance: None,
    };
    ledger.append(event).await
}

async fn emit_loop_completed(
    ledger: &Ledger,
    wih_id: &str,
    iteration: u32,
    outcome: &str,
    evidence_count: usize,
    acceptance_triggered: bool,
) -> Result<String> {
    let payload = json!({
        "wih_id": wih_id,
        "iteration": iteration,
        "completed_at": Utc::now().to_rfc3339(),
        "outcome": outcome,
        "evidence_count": evidence_count,
        "acceptance_refs": acceptance_triggered
    });
    let event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: Some(EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        }),
        r#type: "RailsLoopIterationCompleted".to_string(),
        payload,
        provenance: None,
    };
    ledger.append(event).await
}

async fn emit_loop_spawn_requested(
    ledger: &Ledger,
    wih_id: &str,
    iteration: u32,
) -> Result<String> {
    let payload = json!({
        "wih_id": wih_id,
        "iteration": iteration,
        "requested_at": Utc::now().to_rfc3339(),
    });
    let event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: Some(EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        }),
        r#type: "RailsLoopIterationSpawnRequested".to_string(),
        payload,
        provenance: None,
    };
    ledger.append(event).await
}

fn collect_receipts_after(events: &[AllternitEvent], wih_id: &str, since: &str) -> Vec<String> {
    let since_ts = parse_ts(since);
    events
        .iter()
        .filter(|evt| evt.r#type == "ReceiptWritten")
        .filter(|evt| evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id))
        .filter(|evt| {
            if let Some(ts) = parse_ts(&evt.ts) {
                if let Some(threshold) = since_ts {
                    return ts > threshold;
                }
            }
            since_ts.is_none()
        })
        .filter_map(|evt| {
            evt.payload
                .get("receipt_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

fn collect_all_receipts_for_wih(events: &[AllternitEvent], wih_id: &str) -> Vec<String> {
    events
        .iter()
        .filter(|evt| evt.r#type == "ReceiptWritten")
        .filter(|evt| evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id))
        .filter_map(|evt| {
            evt.payload
                .get("receipt_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

fn parse_ts(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

fn evidence_satisfied(receipts_count: usize, policy: &LoopPolicy) -> bool {
    if policy.required_evidence.is_empty() {
        return true;
    }
    for requirement in &policy.required_evidence {
        match requirement.as_str() {
            "receipt.tool_calls" => {
                if receipts_count == 0 {
                    return false;
                }
            }
            _ => {
                // Unknown evidence requirement is currently treated as satisfied for forward evolution.
            }
        }
    }
    true
}

fn acceptance_refs_satisfied(events: &[AllternitEvent], wih_id: &str, refs: &[String]) -> bool {
    if refs.is_empty() {
        return true;
    }
    let receipts = collect_all_receipts_for_wih(events, wih_id);
    let available: HashSet<_> = receipts.into_iter().collect();
    refs.iter().all(|r| available.contains(r))
}

async fn ensure_loop_policy_injected(
    root: &Path,
    ledger: &Ledger,
    wih_id: &str,
    iteration: u32,
) -> Result<()> {
    let scope = EventScope {
        wih_id: Some(wih_id.to_string()),
        run_id: Some(format!("{}-iter-{}", wih_id, iteration)),
        ..Default::default()
    };
    policy::ensure_injected(root, ledger, Some(scope), "rails-runner").await
}

async fn process_bus_queue(ledger: &Ledger, bus: &Bus) -> Result<()> {
    let pending = bus.poll_pending(32).await?;
    for entry in pending {
        let _ = log_bus_message(ledger, &entry).await?;
        bus.mark_delivered(entry.id).await?;
    }
    Ok(())
}

async fn log_bus_message(ledger: &Ledger, message: &BusMessage) -> Result<String> {
    let scope = derive_scope_from_payload(&message.payload);
    let payload = json!({
        "message_id": message.id,
        "correlation_id": message.correlation_id,
        "to": message.to,
        "from": message.from,
        "kind": message.kind,
        "transport": message.transport,
        "status": message.status,
        "created_at": message.created_at,
        "payload": message.payload
    });
    let event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: Some(scope),
        r#type: "BusMessageProcessed".to_string(),
        payload,
        provenance: None,
    };
    ledger.append(event).await
}

fn derive_scope_from_payload(payload: &Value) -> EventScope {
    let mut scope = EventScope::default();
    if let Some(wih_id) = payload.get("wih_id").and_then(|v| v.as_str()) {
        scope.wih_id = Some(wih_id.to_string());
    }
    if let Some(dag_id) = payload.get("dag_id").and_then(|v| v.as_str()) {
        scope.dag_id = Some(dag_id.to_string());
    }
    scope
}

fn should_spawn_subagent(progress: &LoopProgress, policy: &LoopPolicy) -> bool {
    if policy.spawn_on.is_empty() {
        return false;
    }
    if let Some(spawn_delay) = policy.spawn_delay_secs {
        if let Some(last_spawned) = &progress.last_spawned_at {
            if let Ok(prev) = DateTime::parse_from_rfc3339(last_spawned) {
                if (Utc::now() - prev.with_timezone(&Utc))
                    < ChronoDuration::seconds(spawn_delay as i64)
                {
                    return false;
                }
            }
        }
    }
    progress.spawn_requests.len() < policy.max_iterations as usize
}

async fn emit_loop_escalated(
    ledger: &Ledger,
    wih_id: &str,
    iteration: u32,
    reason: &str,
) -> Result<String> {
    let payload = json!({
        "wih_id": wih_id,
        "iteration": iteration,
        "reason": reason,
        "escalated_at": Utc::now().to_rfc3339()
    });
    let event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "rails-runner".to_string(),
        },
        scope: Some(EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        }),
        r#type: "RailsLoopIterationEscalated".to_string(),
        payload,
        provenance: None,
    };
    ledger.append(event).await
}

async fn resolve_dag(ledger: &Ledger, root: &Path, dag_id: &str) -> Result<DagState> {
    if let Some(dag) = load_dag_view(root, dag_id) {
        return Ok(dag);
    }
    let events = ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag(&events, dag_id);
    Ok(project_dag(&dag_events, dag_id))
}

fn load_dag_view(root: &Path, dag_id: &str) -> Option<DagState> {
    let path = root
        .join(".allternit/work/dags")
        .join(dag_id)
        .join("view")
        .join("dag.current.json");
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn render_dag_markdown(dag: &DagState) -> String {
    let mut out = String::new();
    out.push_str(&format!("# DAG {}\n", dag.dag_id));
    out.push_str("Nodes:\n");
    let mut nodes: Vec<_> = dag.nodes.values().collect();
    nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    for node in nodes {
        out.push_str(&format!(
            "- {} [{}] {}\n",
            node.node_id, node.status, node.title
        ));
    }
    out.push_str("Edges (blocked_by):\n");
    for edge in dag.edges.iter().filter(|e| e.edge_type == "blocked_by") {
        out.push_str(&format!("- {} -> {}\n", edge.from_node_id, edge.to_node_id));
    }
    if !dag.relations.is_empty() {
        out.push_str("Relations:\n");
        for rel in &dag.relations {
            if let Some(note) = &rel.note {
                out.push_str(&format!("- {} ~ {} ({})\n", rel.a, rel.b, note));
            } else {
                out.push_str(&format!("- {} ~ {}\n", rel.a, rel.b));
            }
        }
    }
    out
}

fn collect_dag_ids(events: &[AllternitEvent]) -> Vec<String> {
    let mut dag_ids = HashSet::new();
    for evt in events {
        if let Some(dag_id) = evt.payload.get("dag_id").and_then(|v| v.as_str()) {
            dag_ids.insert(dag_id.to_string());
        }
        if evt.r#type == "DagCreated" {
            if let Some(dag_id) = evt.payload.get("dag_id").and_then(|v| v.as_str()) {
                dag_ids.insert(dag_id.to_string());
            }
        }
    }
    let mut out: Vec<String> = dag_ids.into_iter().collect();
    out.sort();
    out
}

fn events_for_dag(events: &[AllternitEvent], dag_id: &str) -> Vec<AllternitEvent> {
    events
        .iter()
        .filter(|evt| evt.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .cloned()
        .collect()
}

fn active_wih_nodes(events: &[AllternitEvent]) -> HashMap<String, String> {
    let mut wih_nodes: HashMap<String, String> = HashMap::new();
    let mut closed: HashSet<String> = HashSet::new();
    for evt in events {
        if evt.r#type == "WIHCreated" {
            if let (Some(wih_id), Some(node_id)) = (
                evt.payload.get("wih_id").and_then(|v| v.as_str()),
                evt.payload.get("node_id").and_then(|v| v.as_str()),
            ) {
                wih_nodes.insert(wih_id.to_string(), node_id.to_string());
            }
        }
        if evt.r#type == "WIHClosedSigned" || evt.r#type == "WIHArchived" {
            if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                closed.insert(wih_id.to_string());
            }
        }
    }
    let mut active = HashMap::new();
    for (wih_id, node_id) in wih_nodes {
        if !closed.contains(&wih_id) {
            active.insert(node_id, wih_id);
        }
    }
    active
}

fn matches_trace(
    event: &AllternitEvent,
    node_id: Option<&str>,
    wih_id: Option<&str>,
    prompt_id: Option<&str>,
) -> bool {
    if let Some(node_id) = node_id {
        let payload = &event.payload;
        let hits = [
            payload.get("node_id"),
            payload.get("from_node_id"),
            payload.get("to_node_id"),
            payload.get("parent_node_id"),
            payload.get("a"),
            payload.get("b"),
        ]
        .iter()
        .filter_map(|v| v.and_then(|val| val.as_str()))
        .any(|val| val == node_id);
        if !hits {
            return false;
        }
    }
    if let Some(wih_id) = wih_id {
        let payload = &event.payload;
        let hits = payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id);
        if !hits {
            return false;
        }
    }
    if let Some(prompt_id) = prompt_id {
        let payload_match =
            event.payload.get("prompt_id").and_then(|v| v.as_str()) == Some(prompt_id);
        let provenance_match = event
            .provenance
            .as_ref()
            .and_then(|p| p.prompt_id.as_deref())
            == Some(prompt_id);
        if !payload_match && !provenance_match {
            return false;
        }
    }
    true
}

fn project_wih_from_events(
    events: &[AllternitEvent],
    wih_id: &str,
) -> Option<allternit_commrails::wih::types::WihState> {
    let filtered: Vec<AllternitEvent> = events
        .iter()
        .filter(|evt| evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id))
        .cloned()
        .collect();
    project_wih(&filtered, wih_id)
}

/// `<dag_id>/<node_id>` -> (dag_id, node_id).
fn split_node_ref(node: &str) -> Result<(String, String)> {
    match node.split_once('/') {
        Some((d, n)) if !d.is_empty() && !n.is_empty() => Ok((d.to_string(), n.to_string())),
        _ => bail!("--node must be <dag_id>/<node_id>, got {node:?}"),
    }
}

/// `user:<id>` | `agent:<id>` | `<id>` (user).
fn parse_actor(raw: &str) -> Result<Actor> {
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

fn load_mutations(path: Option<PathBuf>, json_inline: Option<String>) -> Result<Vec<DagMutation>> {
    if path.is_some() && json_inline.is_some() {
        anyhow::bail!("use either --mutations or --mutations-json, not both");
    }
    if let Some(path) = path {
        let raw = std::fs::read_to_string(path)?;
        let mutations: Vec<DagMutation> = serde_json::from_str(&raw)?;
        return Ok(mutations);
    }
    if let Some(raw) = json_inline {
        let mutations: Vec<DagMutation> = serde_json::from_str(&raw)?;
        return Ok(mutations);
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod cli_shape_tests {
    use super::Cli;
    use clap::CommandFactory;

    /// clap validates argument shapes lazily, per subcommand, and panics at
    /// run time (e.g. `lease request` had a positional after variadic paths).
    /// Check every subcommand up front.
    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
