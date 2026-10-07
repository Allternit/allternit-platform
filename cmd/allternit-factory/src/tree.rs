//! The command tree (SPEC §7, API.md §2) and what each verb runs.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde_json::json;

use crate::exec::{
    dry_run, fail, not_built, run, run_pane_interactive, run_rails_in_process,
    run_shaped, Code, Ctx, Target,
};

#[derive(Parser)]
#[command(
    name = "allternit-factory",
    version,
    about = "Allternit Factory engine (internal: people run `gizzi`, which runs this)",
    long_about = "Allternit Factory engine (internal: people run `gizzi`, which runs this).\n\n\
                  Exit codes: 0 ok, 1 refused by the Gate, 2 not found (or not built yet), \
                  3 transport broken / engine missing, 4 timeout, 5 needs a person, 64 usage.\n\
                  With --json, stdout is one JSON document; errors are \
                  {\"error\":{\"code\",\"fact\",\"action\"}}.",
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Workspace root holding `.allternit/` (default: the current directory).
    #[arg(long, global = true, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// Print one JSON document on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Top,
}

#[derive(Subcommand)]
pub enum Top {
    /// Run the engine service: HTTP on 127.0.0.1:3011 plus a Unix socket.
    Serve(ServeArgs),
    /// The pane engine's own commands (server, client, workspace, tab, pane,
    /// agent, worktree, session, plugin, integration, …). No arguments opens
    /// the live terminal wall.
    #[command(disable_help_flag = true)]
    Pane {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Bots, harnesses, spawn/stop/recover, machines.
    #[command(subcommand)]
    Agents(AgentsCmd),
    /// Send, capture, transcript, mail, attention, steering, the feed.
    #[command(subcommand)]
    Orchestration(OrchestrationCmd),
    /// Templates, drive, Wake, wait-gates.
    #[command(subcommand)]
    Workflows(WorkflowsCmd),
    /// Campaigns, plans, nodes, judge, approvals.
    #[command(subcommand)]
    Workspace(WorkspaceCmd),
    /// Engine maintenance commands (ledger, index, lease, vault, replay, hook, …).
    #[command(subcommand, hide = true)]
    Internal(InternalCmd),
}

/// Raw arguments handed to an existing implementation.
#[derive(Args, Clone, Default)]
pub struct Rest {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct ServeArgs {
    /// HTTP port (default 3011, or $ALLTERNIT_FACTORY_PORT).
    #[arg(long)]
    pub port: Option<u16>,
    /// HTTP bind host (default 127.0.0.1, or $ALLTERNIT_FACTORY_HOST).
    #[arg(long)]
    pub host: Option<String>,
    /// Unix socket path (default ~/.allternit/factory/factory.sock).
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// Serve HTTP only, no Unix socket.
    #[arg(long)]
    pub no_socket: bool,
    #[command(subcommand)]
    pub surface: Option<ServeSurface>,
}

#[derive(Subcommand)]
pub enum ServeSurface {
    /// The UHP HTTP surface over the pane engine (default 127.0.0.1:8410).
    #[command(disable_help_flag = true)]
    Uhp(Rest),
    /// The fabric node: the loopback shim for a paired cloud computer.
    #[command(disable_help_flag = true)]
    Fabric(Rest),
}

#[derive(Subcommand)]
pub enum AgentsCmd {
    /// Start a team from team.yaml (`--dry-run` prints the plan).
    Up(crate::bots::UpArgs),
    /// Every agent: the session registry reconciled against live panes, plus peers.
    Ps {
        /// Only sessions working under this directory.
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Stop a team (when `<target>` names one) or one agent session (and
    /// optionally remove its worktree).
    Down {
        #[arg(value_name = "TEAM_OR_SLUG")]
        slug: String,
        #[arg(long)]
        rm_worktree: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Who am I, as an agent (inside a factory pane).
    Whoami,
    /// Reconcile the registry with live sessions; respawn dead-but-unfinished
    /// runners with --apply (the default is a dry run).
    Recover {
        slug: Option<String>,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        lead: Option<String>,
        #[arg(long)]
        as_human: bool,
    },
    /// Snapshot the whole team.
    Snapshot(crate::bots::SnapshotArgs),
    /// Restore a team snapshot (the latest by default).
    Restore(crate::bots::RestoreArgs),
    /// Set a bot's model.
    Model(Planned),
    /// Hand a bot's seat to a fresh session.
    Handoff(Planned),
    /// Harness status / sync / install / uninstall (`--dry-run` supported).
    #[command(disable_help_flag = true)]
    Harness(Rest),
    /// Pack a team for sharing.
    Pack(crate::bots::PackArgs),
    /// Install a packed team (a pack, a folder, or a pinned GitHub tree URL).
    Install(crate::bots::InstallArgs),
    /// Bots in allternit-api: add.
    #[command(subcommand)]
    Bot(crate::bots::BotCmd),
    /// Agent templates.
    Templates(Planned),
    /// Open the live terminal wall of running agents.
    Wall { team: Option<String> },
    /// Attach to one agent terminal (by terminal id).
    Attach { terminal: String },
    /// Check transport, tools and harnesses.
    Doctor,
}

/// Arguments of a verb that is not built yet (accepted, then refused honestly).
#[derive(Args)]
pub struct Planned {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Subcommand)]
pub enum OrchestrationCmd {
    /// Send text to an agent: verified delivery, or queued with --queue.
    Send {
        to: String,
        text: Vec<String>,
        /// Queue to the agent's mailbox instead of typing now.
        #[arg(long)]
        queue: bool,
        /// Read the text from a file.
        #[arg(short = 'f', long, conflicts_with = "text")]
        file: Option<PathBuf>,
        /// The bot thread this message belongs to.
        #[arg(long)]
        thread: Option<String>,
        /// The node this message is about.
        #[arg(long)]
        node: Option<String>,
        /// The DAG of --node.
        #[arg(long)]
        dag: Option<String>,
        /// Idempotency key: the same key twice returns the first delivery.
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// The last lines of an agent's terminal.
    Capture { to: String, lines: Option<u32> },
    /// An agent's recorded transcript.
    Transcript {
        to: String,
        #[arg(long)]
        tail: Option<usize>,
    },
    /// Deliver an agent's queued messages (oldest first, each only once its
    /// paste is verified).
    Drain {
        to: String,
        /// Every queued message, not just the oldest.
        #[arg(long)]
        all: bool,
        /// List what is queued; deliver nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Threads (standing / task).
    Threads(Planned),
    /// Mail: list, read, send, decide, … (`--help` for the full set).
    #[command(disable_help_flag = true)]
    Mail(Rest),
    /// The ledger event feed (newest N, default 50).
    Feed { n: Option<usize> },
    /// The attention gate: list, submit, release, ack.
    #[command(disable_help_flag = true)]
    Attention(Rest),
    /// Steering: checkpoint, consult, commit-gate.
    #[command(disable_help_flag = true)]
    Steer(Rest),
    /// Ask the Coordinator to run a project.
    Coordinate(Planned),
}

#[derive(Subcommand)]
pub enum WorkflowsCmd {
    /// Plan a DAG from a template.
    Run {
        template: String,
        /// Template parameter `name=value` (repeatable).
        #[arg(long = "param")]
        params: Vec<String>,
        /// Plan text / title.
        #[arg(long)]
        text: Option<String>,
        /// The person's words: the plan text, and the `intent` param when the template has one.
        #[arg(long)]
        intent: Option<String>,
        /// Team whose bots take the template's `role:` steps (team.yaml roles).
        #[arg(long)]
        team: Option<String>,
        /// Preset of --team.
        #[arg(long, requires = "team")]
        preset: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run a DAG's READY nodes in the foreground (`--once`, `--dry-run`, …).
    #[command(disable_help_flag = true)]
    Drive(Rest),
    /// Templates: list, show, check.
    #[command(subcommand)]
    Template(TemplateCmd),
    /// The Wake queue: list, due, run-due, cancel.
    #[command(disable_help_flag = true)]
    Wake(Rest),
    /// Wait-gates on nodes: add, resolve, list, pending.
    #[command(disable_help_flag = true)]
    Gate(Rest),
    /// Render a DAG run (JSON by default).
    Status {
        run: String,
        #[arg(long, default_value = "json")]
        format: String,
    },
}

#[derive(Subcommand)]
pub enum TemplateCmd {
    /// Every plan template in the workspace.
    List,
    /// One template.
    Show { template: String },
    /// Parse a template (id or file) and report its parameters.
    Check { template: String },
    /// Check a template file and save it to this workspace (--id, --force, --dry-run).
    Save(Planned),
}

#[derive(Subcommand)]
pub enum WorkspaceCmd {
    /// Campaigns: new, list, status, pause, resume, finish, … .
    #[command(disable_help_flag = true)]
    Campaign(Rest),
    /// Plans: new, refine, show.
    #[command(disable_help_flag = true)]
    Plan(Rest),
    /// DAG nodes: add, list, claim, handoff, close.
    #[command(subcommand)]
    Node(NodeCmd),
    /// Approve a node: resolve its wait-gate, or record the judge's human decision.
    Approve {
        /// Node (`<dag>/<node>` for a wait-gate, the node id for --judge).
        node: String,
        /// Wait-gate id to resolve as ok.
        gate: Option<String>,
        /// Record a human judge decision (accomplished) instead of a wait-gate.
        #[arg(long, conflicts_with = "gate")]
        judge: bool,
        #[arg(long)]
        actor: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Proof: add, show.
    #[command(subcommand)]
    Proof(crate::work::ProofCmd),
    /// The judge: show, resolve, policy, pending, … .
    #[command(disable_help_flag = true)]
    Judge(Rest),
    /// The project board of a campaign (or a DAG id).
    Board { campaign: String },
    /// The cowork task queue.
    Tasks(Planned),
}

#[derive(Subcommand)]
pub enum NodeCmd {
    /// Add a node to a DAG (`--dag --parent --title …`).
    #[command(disable_help_flag = true)]
    Add(Rest),
    /// One node's page: card, spec, progress, proof, deliveries, WIH.
    Show { dag: String, node: String },
    /// Work items: every open node, or the READY ones.
    List {
        #[arg(long)]
        dag: Option<String>,
        #[arg(long)]
        ready: bool,
        /// Only mine (needs `agents whoami`).
        #[arg(long)]
        mine: bool,
    },
    /// Claim a node (`<node> --dag D --agent A`).
    #[command(disable_help_flag = true)]
    Claim(Rest),
    /// Hand a claimed node to another agent.
    Handoff(Planned),
    /// Close a claimed node (`<wih> <status> [evidence…]`).
    #[command(disable_help_flag = true)]
    Close(Rest),
}

#[derive(Subcommand)]
pub enum InternalCmd {
    /// The maintenance CLI with its own exit codes (hooks rely on them).
    #[command(disable_help_flag = true)]
    Core(Rest),
    /// The maintenance CLI with API.md exit codes (used by the part verbs).
    #[command(disable_help_flag = true, hide = true)]
    VerbCore(Rest),
    /// The spawn-gate hooks a gated harness runs (`--root R claude-pretool …`,
    /// `spawn-check`, `claude-settings`); the same as `core hook …`.
    #[command(disable_help_flag = true)]
    Hook(Rest),
    /// Move the old agent-orchestrator home into ~/.allternit/factory once
    /// (`serve` does this at start). Never overwrites; reports conflicts.
    MigrateHome(MigrateHomeArgs),
}

#[derive(Args)]
pub struct MigrateHomeArgs {
    /// Print what would move; change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Run even if the marker says it ran before (moves whatever reappeared).
    #[arg(long)]
    pub again: bool,
}

/// Pull `--json`, `--dry-run` and `--root DIR` out of raw passthrough args
/// (clap's global flags don't reach past a trailing argument list).
struct Split {
    args: Vec<String>,
    json: bool,
    dry_run: bool,
    root: Option<PathBuf>,
}

fn split(rest: Vec<String>, keep_dry_run: bool) -> Split {
    let mut out = Split { args: Vec::new(), json: false, dry_run: false, root: None };
    let mut iter = rest.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => out.json = true,
            "--dry-run" if !keep_dry_run => out.dry_run = true,
            "--root" => out.root = iter.next().map(PathBuf::from),
            _ => {
                if let Some(root) = arg.strip_prefix("--root=") {
                    out.root = Some(PathBuf::from(root));
                } else {
                    out.args.push(arg);
                }
            }
        }
    }
    out
}

/// Run a passthrough rails verb: `prefix` + the user's args.
fn rails_passthrough(ctx: &Ctx, prefix: &[&str], rest: Rest, native_dry_run: bool) -> u8 {
    let split = split(rest.args, native_dry_run);
    let ctx = Ctx::new(split.root.or_else(|| ctx.root.clone()), ctx.json || split.json);
    let mut args: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
    args.extend(split.args);
    let target = Target::Rails(args);
    if split.dry_run {
        return dry_run(&ctx, &target);
    }
    run(&ctx, target)
}

/// `workspace node handoff <dag>/<node> --to <agent> [--note <text>] [--dry-run]`:
/// hand a claimed node to another agent through the Gate, note it in the
/// node's PROGRESS.md, and tell a local team bot (`bot@team`) it owns it now.
fn node_handoff(ctx: &Ctx, args: Vec<String>) -> u8 {
    let mut out: Vec<String> = vec![];
    let mut target: Option<String> = None;
    let mut to: Option<String> = None;
    let mut dry = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => {
                let v = it.next().unwrap_or_default();
                to = Some(v.clone());
                out.push(a);
                out.push(v);
            }
            "--dry-run" => {
                dry = true;
                out.push(a);
            }
            _ if a.starts_with("--") => out.push(a),
            _ if target.is_none() && a.contains('/') => target = Some(a),
            _ => out.push(a),
        }
    }
    let (Some(target), Some(to)) = (target, to) else {
        return fail(ctx, Code::Usage, "node handoff needs <dag>/<node> and --to <agent>", Some("allternit-factory workspace node handoff <dag>/<node> --to <bot@team> [--note <text>]"));
    };
    let (dag, node) = target.split_once('/').unwrap_or_default();
    let mut rest = vec![node.to_string(), "--dag".to_string(), dag.to_string()];
    rest.extend(out);
    let code = rails_passthrough(ctx, &["wih", "handoff"], Rest { args: rest }, true);
    if code != 0 || dry || !to.contains('@') {
        return code;
    }
    // Tell the new owner, if it's a bot on this computer.
    let folder = format!(".allternit/work/dags/{dag}/nodes/{node}/");
    let text = format!("You now own node {dag}/{node}. Read {folder}SPEC.md and PROGRESS.md (the handoff note is at the end), then continue.");
    if crate::part::send_quiet(ctx, &to, text, Some(node.to_string()), Some(dag.to_string())).is_none() && !ctx.json {
        eprintln!("note: couldn't message {to} on this computer; tell it with `gizzi orchestration send {to} \"…\"`");
    }
    0
}

fn opt(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value);
    }
}

pub fn dispatch(ctx: &Ctx, command: Top) -> u8 {
    match command {
        Top::Serve(args) => serve(ctx, args),
        Top::Pane { args } => run_pane_interactive(args),
        Top::Agents(cmd) => agents(ctx, cmd),
        Top::Orchestration(cmd) => orchestration(ctx, cmd),
        Top::Workflows(cmd) => workflows(ctx, cmd),
        Top::Workspace(cmd) => workspace(ctx, cmd),
        Top::Internal(InternalCmd::Core(rest)) => run_rails_in_process(ctx.root.as_ref(), rest.args, false),
        Top::Internal(InternalCmd::VerbCore(rest)) => run_rails_in_process(ctx.root.as_ref(), rest.args, true),
        Top::Internal(InternalCmd::MigrateHome(args)) => migrate_home(ctx, args),
        Top::Internal(InternalCmd::Hook(rest)) => {
            let split = split(rest.args, true);
            let mut args = vec!["hook".to_string()];
            args.extend(split.args);
            run_rails_in_process(split.root.as_ref().or(ctx.root.as_ref()), args, false)
        }
    }
}

/// `new`, else the pre-Factory names (a user's shell or launchd job may still
/// export them), with a one-time deprecation on stderr.
fn env_pref(new: &str, old: &[&str]) -> Option<String> {
    if let Some(v) = std::env::var(new).ok().filter(|v| !v.is_empty()) {
        return Some(v);
    }
    for name in old {
        if let Some(v) = std::env::var(name).ok().filter(|v| !v.is_empty()) {
            eprintln!("allternit-factory: {name} is deprecated; set {new} instead");
            return Some(v);
        }
    }
    None
}

fn migrate_home(ctx: &Ctx, args: MigrateHomeArgs) -> u8 {
    use allternit_factory_engine::agents::home_migrate;
    let from = home_migrate::legacy_home();
    let to = allternit_factory_engine::agents::registry::factory_home();
    if !args.again && !args.dry_run {
        if let Some(prev) = home_migrate::read_marker(&to) {
            if !from.is_dir() {
                return emit_migration(ctx, &prev, "already moved");
            }
        }
    }
    match home_migrate::migrate(&from, &to, args.dry_run) {
        Ok(report) => emit_migration(ctx, &report, if args.dry_run { "would move" } else { "moved" }),
        Err(err) => fail(ctx, Code::Internal, &format!("{err:#}"), Some("Fix the path named above and run it again; nothing was overwritten.")),
    }
}

fn emit_migration(ctx: &Ctx, report: &allternit_factory_engine::agents::home_migrate::MigrationReport, verb: &str) -> u8 {
    if ctx.json {
        return crate::exec::ok_json(serde_json::to_value(report).unwrap_or_default());
    }
    println!(
        "{verb} {} → {}: {} moved, {} identical, {} conflicts{}",
        report.from,
        report.to,
        report.moved.len(),
        report.deduplicated.len(),
        report.conflicts.len(),
        if report.removed_old_home { "; old folder removed" } else { "" }
    );
    for c in &report.conflicts {
        println!("  conflict (left in place): {c}");
    }
    0
}

/// The automatic move at `serve` start. Logged, never fatal.
fn migrate_home_at_start() {
    use allternit_factory_engine::agents::home_migrate::{migrate_default_home, Outcome};
    match migrate_default_home() {
        Ok(Outcome::Migrated(r)) => eprintln!(
            "allternit-factory: moved {} into {} ({} moved, {} identical, {} conflicts left in place)",
            r.from,
            r.to,
            r.moved.len(),
            r.deduplicated.len(),
            r.conflicts.len()
        ),
        Ok(_) => {}
        Err(err) => eprintln!("allternit-factory: the one-time home move failed (the engine keeps running): {err:#}"),
    }
}

fn serve(ctx: &Ctx, args: ServeArgs) -> u8 {
    match args.surface {
        Some(ServeSurface::Uhp(rest)) => {
            let mut argv = vec!["serve".to_string()];
            argv.extend(rest.args);
            return run_pane_interactive(argv);
        }
        Some(ServeSurface::Fabric(rest)) => {
            let mut argv = vec!["fabric".to_string(), "serve".to_string()];
            argv.extend(rest.args);
            return run_pane_interactive(argv);
        }
        None => {}
    }
    let host = args
        .host
        .or_else(|| env_pref("ALLTERNIT_FACTORY_HOST", &["ALLTERNIT_COMMRAILS_HOST", "ALLTERNIT_RAILS_HOST"])) // old-names: keep (deprecated env read)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = match args.port {
        Some(port) => port.to_string(),
        None => env_pref("ALLTERNIT_FACTORY_PORT", &["ALLTERNIT_COMMRAILS_PORT", "ALLTERNIT_RAILS_PORT"]) // old-names: keep (deprecated env read)
            .unwrap_or_else(|| "3011".to_string()),
    };
    let root = ctx
        .root
        .clone()
        .or_else(|| env_pref("ALLTERNIT_FACTORY_ROOT", &["ALLTERNIT_COMMRAILS_ROOT", "ALLTERNIT_RAILS_ROOT"]).map(PathBuf::from)) // old-names: keep (deprecated env read)
        .unwrap_or_else(|| ctx.root_dir());
    let socket = if args.no_socket {
        None
    } else {
        match args.socket {
            Some(path) => Some(path),
            None => match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
                Some(home) => Some(PathBuf::from(home).join(".allternit/factory/factory.sock")),
                None => {
                    return fail(
                        ctx,
                        Code::Usage,
                        "HOME is not set, so the default socket path is unknown",
                        Some("Pass --socket <path> or --no-socket."),
                    )
                }
            },
        }
    };
    let bind = format!("{host}:{port}");
    let _ = tracing_subscriber::fmt::try_init();
    migrate_home_at_start();
    eprintln!(
        "allternit-factory: serving {} on http://{bind}{}",
        root.display(),
        socket
            .as_ref()
            .map(|s| format!(" and {}", s.display()))
            .unwrap_or_default()
    );
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(err) => return fail(ctx, Code::Internal, &format!("async runtime: {err}"), None),
    };
    match runtime.block_on(allternit_factory_engine::service::run_service_on(
        &bind,
        socket.as_deref(),
        root,
    )) {
        Ok(()) => 0,
        Err(err) => {
            let text = format!("{err:#}");
            let code = if text.contains("in use") || text.contains("already listening") {
                Code::Refused
            } else {
                Code::Internal
            };
            fail(ctx, code, &text, None)
        }
    }
}

fn agents(ctx: &Ctx, cmd: AgentsCmd) -> u8 {
    const TEAM: &str = "Not in the engine yet; edit team.yaml and run `agents up <team>` again.";
    match cmd {
        AgentsCmd::Up(args) => crate::bots::up(ctx, args),
        AgentsCmd::Whoami => crate::bots::whoami_cmd(ctx),
        AgentsCmd::Snapshot(args) => crate::bots::snapshot_cmd(ctx, args),
        AgentsCmd::Restore(args) => crate::bots::restore(ctx, args),
        AgentsCmd::Pack(args) => crate::bots::pack(ctx, args),
        AgentsCmd::Install(args) => crate::bots::install(ctx, args),
        AgentsCmd::Bot(cmd) => crate::bots::bot(ctx, cmd),
        AgentsCmd::Model(p) => crate::part::model(ctx, &p.args),
        AgentsCmd::Handoff(p) => crate::part::handoff(ctx, &p.args),
        AgentsCmd::Templates(_) => not_built(ctx, "agents", "templates", TEAM),
        AgentsCmd::Ps { cwd } => crate::part::ps(ctx, cwd),
        AgentsCmd::Down { slug, rm_worktree, dry_run: dry } => {
            if let Some(code) = crate::bots::down_team(ctx, &slug, rm_worktree, dry) {
                return code;
            }
            crate::part::down(ctx, slug, rm_worktree, dry)
        }
        AgentsCmd::Recover { slug, apply, dry_run: dry, lead, as_human } => {
            if apply && dry {
                return fail(ctx, Code::Usage, "--apply and --dry-run contradict each other", None);
            }
            crate::part::recover(ctx, slug, apply, lead, as_human)
        }
        AgentsCmd::Harness(rest) => {
            let split = split(rest.args, true);
            let ctx = Ctx::new(ctx.root.clone(), ctx.json || split.json);
            let mut args = vec!["harness".to_string()];
            args.extend(split.args);
            run(&ctx, Target::Pane(args))
        }
        AgentsCmd::Doctor => run(ctx, Target::Pane(vec!["doctor".into()])),
        AgentsCmd::Wall { team: Some(_) } => not_built(
            ctx,
            "agents",
            "wall",
            "A team-scoped wall needs teams (team.yaml); run `agents wall` with no team for every running agent.",
        ),
        AgentsCmd::Wall { team: None } => {
            if ctx.json {
                return fail(ctx, Code::Usage, "agents wall is interactive and has no JSON form", None);
            }
            run_pane_interactive(vec!["--session".into(), "ao".into()])
        }
        AgentsCmd::Attach { terminal } => {
            if ctx.json {
                return fail(ctx, Code::Usage, "agents attach is interactive and has no JSON form", None);
            }
            run_pane_interactive(vec![
                "--session".into(),
                "ao".into(),
                "terminal".into(),
                "attach".into(),
                terminal,
            ])
        }
    }
}

fn orchestration(ctx: &Ctx, cmd: OrchestrationCmd) -> u8 {
    match cmd {
        OrchestrationCmd::Send { to, text, queue, file, thread, node, dag, key, dry_run: dry } => {
            let text = match file {
                Some(file) => match std::fs::read_to_string(&file) {
                    Ok(t) => t.trim_end_matches('\n').to_string(),
                    Err(e) => return fail(ctx, Code::NotFound, &format!("reading {}: {e}", file.display()), None),
                },
                None if text.is_empty() => return fail(ctx, Code::Usage, "send needs text or -f <file>", None),
                None => text.join(" "),
            };
            crate::part::send(ctx, to, text, queue, thread, node, dag, key, dry)
        }
        OrchestrationCmd::Capture { to, lines } => crate::part::capture(ctx, to, lines),
        OrchestrationCmd::Transcript { to, tail } => crate::part::transcript(ctx, to, tail),
        OrchestrationCmd::Drain { to, all, dry_run: dry } => crate::part::drain(ctx, to, all, dry),
        // Threads and the Coordinator live with the account in allternit-api,
        // so Gizzi serves them with the person's own sign-in.
        OrchestrationCmd::Threads(_) => fail(
            ctx,
            Code::Usage,
            "orchestration threads lives with your account, not in this workspace",
            Some("Use `gizzi orchestration threads list|show|new|steer|resolve`."),
        ),
        OrchestrationCmd::Coordinate(_) => fail(
            ctx,
            Code::Usage,
            "orchestration coordinate lives with your account, not in this workspace",
            Some("Use `gizzi orchestration coordinate <project> \"…\"`."),
        ),
        OrchestrationCmd::Mail(rest) => rails_passthrough(ctx, &["mail"], rest, false),
        OrchestrationCmd::Attention(rest) => rails_passthrough(ctx, &["attention"], rest, false),
        OrchestrationCmd::Steer(rest) => rails_passthrough(ctx, &["steer"], rest, false),
        OrchestrationCmd::Feed { n } => {
            let mut args = vec!["ledger".to_string(), "tail".to_string()];
            args.extend(n.map(|n| n.to_string()));
            run(ctx, Target::Rails(args))
        }
    }
}

fn workflows(ctx: &Ctx, cmd: WorkflowsCmd) -> u8 {
    match cmd {
        WorkflowsCmd::Run { template, params, text, intent, team, preset, dry_run: dry } => {
            let mut run_args = crate::work::RunArgs { template, params, text, intent, team, preset, dry_run: dry };
            if let Some(code) = crate::work::run(ctx, &mut run_args) {
                return code;
            }
            let crate::work::RunArgs { template, params, text, .. } = run_args;
            let mut args = vec!["plan".to_string(), "new".to_string()];
            args.extend(text);
            args.push("--template".into());
            args.push(template);
            for p in params {
                args.push("--param".into());
                args.push(p);
            }
            let target = Target::Rails(args);
            if dry {
                return dry_run(ctx, &target);
            }
            run(ctx, target)
        }
        WorkflowsCmd::Drive(rest) => {
            if let Some(code) = crate::work::drive_precheck(ctx, &rest.args) {
                return code;
            }
            rails_passthrough(ctx, &["drive"], rest, true)
        }
        WorkflowsCmd::Wake(rest) => rails_passthrough(ctx, &["wake"], rest, false),
        WorkflowsCmd::Gate(rest) => rails_passthrough(ctx, &["wait-gate"], rest, false),
        WorkflowsCmd::Status { run: dag, format } => run(
            ctx,
            Target::Rails(vec!["dag".into(), "render".into(), dag, "--format".into(), format]),
        ),
        WorkflowsCmd::Template(cmd) => template(ctx, cmd),
    }
}

fn template(ctx: &Ctx, cmd: TemplateCmd) -> u8 {
    match cmd {
        TemplateCmd::Save(p) => crate::work::template_save(ctx, &p.args),
        TemplateCmd::List => crate::work::template(ctx, "list", None),
        TemplateCmd::Show { template } => crate::work::template(ctx, "show", Some(&template)),
        TemplateCmd::Check { template } => crate::work::template(ctx, "check", Some(&template)),
    }
}

fn workspace(ctx: &Ctx, cmd: WorkspaceCmd) -> u8 {
    match cmd {
        WorkspaceCmd::Campaign(mut rest) => {
            // SPEC verb `new` is the engine's `declare`.
            if rest.args.first().map(String::as_str) == Some("new") {
                rest.args[0] = "declare".into();
            }
            rails_passthrough(ctx, &["campaign"], rest, false)
        }
        WorkspaceCmd::Plan(rest) => rails_passthrough(ctx, &["plan"], rest, false),
        WorkspaceCmd::Judge(rest) => rails_passthrough(ctx, &["judge"], rest, false),
        WorkspaceCmd::Node(NodeCmd::Add(rest)) => rails_passthrough(ctx, &["node", "add"], rest, false),
        WorkspaceCmd::Node(NodeCmd::Show { dag, node }) => crate::work::node_show(ctx, &dag, &node),
        WorkspaceCmd::Node(NodeCmd::Claim(rest)) => rails_passthrough(ctx, &["wih", "pickup"], rest, false),
        WorkspaceCmd::Node(NodeCmd::Close(rest)) => rails_passthrough(ctx, &["wih", "close"], rest, false),
        WorkspaceCmd::Node(NodeCmd::Handoff(p)) => node_handoff(ctx, p.args),
        WorkspaceCmd::Node(NodeCmd::List { dag, ready, mine }) => {
            if mine {
                return crate::work::node_list_mine(ctx);
            }
            let mut args = vec!["wih".to_string(), "list".to_string()];
            if ready {
                args.push("--ready".into());
            }
            opt(&mut args, "--dag", dag);
            let shape: fn(&str) -> serde_json::Value =
                if ready { node_list_ready_json } else { node_list_claimed_json };
            run_shaped(ctx, Target::Rails(args), Some(shape))
        }
        WorkspaceCmd::Approve { node, gate, judge, actor, reason, dry_run: dry } => {
            let target = if judge {
                let Some(actor) = actor else {
                    return fail(ctx, Code::Usage, "approve --judge needs --actor <who>", None);
                };
                let mut args = vec![
                    "judge".to_string(),
                    "resolve".into(),
                    node,
                    "accomplished".into(),
                    "--actor".into(),
                    actor,
                ];
                opt(&mut args, "--reason", reason);
                Target::Rails(args)
            } else {
                let Some(gate) = gate else {
                    return fail(
                        ctx,
                        Code::Usage,
                        "approve needs the wait-gate id, or --judge for the judge's human decision",
                        Some("List open gates with `allternit-factory workflows gate pending`."),
                    );
                };
                let mut args = vec![
                    "wait-gate".to_string(),
                    "resolve".into(),
                    "--node".into(),
                    node,
                    gate,
                    "--outcome".into(),
                    "ok".into(),
                ];
                opt(&mut args, "--actor", actor);
                opt(&mut args, "--reason", reason);
                Target::Rails(args)
            };
            if dry {
                return dry_run(ctx, &target);
            }
            run(ctx, target)
        }
        WorkspaceCmd::Proof(cmd) => crate::work::proof_cmd(ctx, cmd),
        WorkspaceCmd::Board { campaign } => crate::work::board_cmd(ctx, &campaign),
        // Tasks live with the account in allternit-api (`/api/factory/tasks/*`),
        // not in this workspace's ledger, so Gizzi serves them with the
        // person's own sign-in.
        WorkspaceCmd::Tasks(_) => fail(
            ctx,
            Code::Usage,
            "workspace tasks lives with your account, not in this workspace",
            Some("Use `gizzi workspace tasks board` (or `tasks`, `queue`); the app shows the same board."),
        ),
    }
}

/// `wih list --ready` prints `<dag> <node> <title…>` per READY node.
fn node_list_ready_json(text: &str) -> serde_json::Value {
    let nodes: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(3, ' ');
            let dag = parts.next().filter(|s| !s.is_empty())?;
            let node = parts.next()?;
            let title = parts.next().unwrap_or("");
            Some(json!({ "dag": dag, "node": node, "title": title, "state": "ready" }))
        })
        .collect();
    json!({ "nodes": nodes })
}

/// `wih list` prints `<wih> <node>` per claimed (open) node.
fn node_list_claimed_json(text: &str) -> serde_json::Value {
    let nodes: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let wih = parts.next()?;
            let node = parts.next()?;
            Some(json!({ "wih": wih, "node": node, "state": "claimed" }))
        })
        .collect();
    json!({ "nodes": nodes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_list_lines_become_json() {
        assert_eq!(
            node_list_ready_json("d1 n2 Fix the build\n\n"),
            json!({ "nodes": [{ "dag": "d1", "node": "n2", "title": "Fix the build", "state": "ready" }] })
        );
        assert_eq!(
            node_list_claimed_json("wih_1 n2\n"),
            json!({ "nodes": [{ "wih": "wih_1", "node": "n2", "state": "claimed" }] })
        );
        assert_eq!(node_list_claimed_json(""), json!({ "nodes": [] }));
    }
}
