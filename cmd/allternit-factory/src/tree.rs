//! The command tree (SPEC §7, API.md §2) and what each verb runs.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde_json::json;

use crate::exec::{
    dry_run, fail, not_built, ok_json, run, run_pane_interactive, run_rails_in_process,
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
    /// HTTP port (default 3011, or $ALLTERNIT_COMMRAILS_PORT).
    #[arg(long)]
    pub port: Option<u16>,
    /// HTTP bind host (default 127.0.0.1, or $ALLTERNIT_COMMRAILS_HOST).
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
    /// Start a team from team.yaml.
    Up(Planned),
    /// Every agent: the session registry reconciled against live panes, plus peers.
    Ps {
        /// Only sessions working under this directory.
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Stop an agent session (and optionally remove its worktree).
    Down {
        slug: String,
        #[arg(long)]
        rm_worktree: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Who am I, as an agent.
    Whoami(Planned),
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
    Snapshot(Planned),
    /// Restore a team snapshot.
    Restore(Planned),
    /// Set a bot's model.
    Model(Planned),
    /// Hand a bot's seat to a fresh session.
    Handoff(Planned),
    /// Harness status / sync / install / uninstall (`--dry-run` supported).
    #[command(disable_help_flag = true)]
    Harness(Rest),
    /// Pack a team for sharing.
    Pack(Planned),
    /// Install a packed team.
    Install(Planned),
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
    /// Save a DAG as a template.
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
    Proof(Planned),
    /// The judge: show, resolve, policy, pending, … .
    #[command(disable_help_flag = true)]
    Judge(Rest),
    /// The project board.
    Board(Planned),
    /// The cowork task queue.
    Tasks(Planned),
}

#[derive(Subcommand)]
pub enum NodeCmd {
    /// Add a node to a DAG (`--dag --parent --title …`).
    #[command(disable_help_flag = true)]
    Add(Rest),
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
    Rails(Rest),
    /// The maintenance CLI with API.md exit codes (used by the part verbs).
    #[command(disable_help_flag = true, hide = true)]
    VerbRails(Rest),
    /// The spawn-gate hooks a gated harness runs (`--root R claude-pretool …`,
    /// `spawn-check`, `claude-settings`); the same as `rails hook …`.
    #[command(disable_help_flag = true)]
    Hook(Rest),
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
        Top::Internal(InternalCmd::Rails(rest)) => run_rails_in_process(ctx.root.as_ref(), rest.args, false),
        Top::Internal(InternalCmd::VerbRails(rest)) => run_rails_in_process(ctx.root.as_ref(), rest.args, true),
        Top::Internal(InternalCmd::Hook(rest)) => {
            let split = split(rest.args, true);
            let mut args = vec!["hook".to_string()];
            args.extend(split.args);
            run_rails_in_process(split.root.as_ref().or(ctx.root.as_ref()), args, false)
        }
    }
}

fn env_pref(new: &str, old: &str) -> Option<String> {
    std::env::var(new).ok().or_else(|| std::env::var(old).ok())
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
        .or_else(|| env_pref("ALLTERNIT_COMMRAILS_HOST", "ALLTERNIT_RAILS_HOST"))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = match args.port {
        Some(port) => port.to_string(),
        None => env_pref("ALLTERNIT_COMMRAILS_PORT", "ALLTERNIT_RAILS_PORT")
            .unwrap_or_else(|| "3011".to_string()),
    };
    let root = ctx
        .root
        .clone()
        .or_else(|| env_pref("ALLTERNIT_COMMRAILS_ROOT", "ALLTERNIT_RAILS_ROOT").map(PathBuf::from))
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
    const TEAM: &str = "Teams (team.yaml) are not in the engine yet; spawn single agents with `allternit-factory pane spawn`.";
    match cmd {
        AgentsCmd::Up(_) => not_built(ctx, "agents", "up", TEAM),
        AgentsCmd::Whoami(_) => not_built(ctx, "agents", "whoami", TEAM),
        AgentsCmd::Snapshot(_) => not_built(ctx, "agents", "snapshot", TEAM),
        AgentsCmd::Restore(_) => not_built(ctx, "agents", "restore", TEAM),
        AgentsCmd::Model(_) => not_built(ctx, "agents", "model", TEAM),
        AgentsCmd::Handoff(_) => not_built(ctx, "agents", "handoff", TEAM),
        AgentsCmd::Pack(_) => not_built(ctx, "agents", "pack", TEAM),
        AgentsCmd::Install(_) => not_built(ctx, "agents", "install", TEAM),
        AgentsCmd::Templates(_) => not_built(ctx, "agents", "templates", TEAM),
        AgentsCmd::Ps { cwd } => crate::part::ps(ctx, cwd),
        AgentsCmd::Down { slug, rm_worktree, dry_run: dry } => {
            let mut args = vec!["kill".to_string(), slug];
            if rm_worktree {
                args.push("--rm-worktree".into());
            }
            let target = Target::Pane(args);
            if dry {
                return dry_run(ctx, &target);
            }
            run(ctx, target)
        }
        AgentsCmd::Recover { slug, apply, dry_run: dry, lead, as_human } => {
            if apply && dry {
                return fail(ctx, Code::Usage, "--apply and --dry-run contradict each other", None);
            }
            let mut args = vec!["recover".to_string()];
            args.extend(slug);
            if apply {
                args.push("--apply".into());
            }
            opt(&mut args, "--lead", lead);
            if as_human {
                args.push("--as-human".into());
            }
            run(ctx, Target::Pane(args))
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
        OrchestrationCmd::Capture { to, lines } => {
            let mut args = vec!["status".to_string(), to];
            args.extend(lines.map(|n| n.to_string()));
            run(ctx, Target::Pane(args))
        }
        OrchestrationCmd::Transcript { to, tail } => {
            let mut args = vec!["transcript".to_string(), to];
            opt(&mut args, "--tail", tail.map(|n| n.to_string()));
            run(ctx, Target::Pane(args))
        }
        OrchestrationCmd::Threads(_) => not_built(
            ctx,
            "orchestration",
            "threads",
            "Threads live in Bot Mode today; the engine API for them is not built.",
        ),
        OrchestrationCmd::Coordinate(_) => not_built(
            ctx,
            "orchestration",
            "coordinate",
            "The Coordinator is not wired to the engine yet.",
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
        WorkflowsCmd::Run { template, params, text, dry_run: dry } => {
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
        WorkflowsCmd::Drive(rest) => rails_passthrough(ctx, &["drive"], rest, true),
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
    use allternit_factory_engine::templates::{TemplateStore, TEMPLATE_DIR};
    let root = ctx.root_dir();
    let store = || -> Result<Option<TemplateStore>, String> {
        // Reading never creates the template directory.
        if !root.join(TEMPLATE_DIR).is_dir() {
            return Ok(None);
        }
        TemplateStore::new(&root).map(Some).map_err(|e| format!("{e:#}"))
    };
    match cmd {
        TemplateCmd::Save(_) => not_built(
            ctx,
            "workflows",
            "template save",
            "Write the template file under .allternit/rails/templates/ by hand for now.",
        ),
        TemplateCmd::List => {
            let templates = match store() {
                Ok(Some(store)) => match store.list() {
                    Ok(list) => list,
                    Err(e) => return fail(ctx, Code::Internal, &format!("{e:#}"), None),
                },
                Ok(None) => Vec::new(),
                Err(e) => return fail(ctx, Code::Internal, &e, None),
            };
            if ctx.json {
                return ok_json(json!({ "templates": templates }));
            }
            if templates.is_empty() {
                println!("no templates in {}", root.join(TEMPLATE_DIR).display());
            }
            for t in &templates {
                println!("{}\t{}\t{} steps", t.id, t.name, t.steps.len());
            }
            0
        }
        TemplateCmd::Show { template } | TemplateCmd::Check { template } => {
            let resolved = match store() {
                Ok(Some(store)) => store.resolve(&template),
                Ok(None) if std::path::Path::new(&template).is_file() => {
                    TemplateStore::load_file(std::path::Path::new(&template))
                }
                Ok(None) => {
                    return fail(
                        ctx,
                        Code::NotFound,
                        &format!("template {template} not found (no {} here)", TEMPLATE_DIR),
                        None,
                    )
                }
                Err(e) => return fail(ctx, Code::Internal, &e, None),
            };
            match resolved {
                Ok(t) => {
                    if ctx.json {
                        return ok_json(serde_json::to_value(&t).unwrap_or_default());
                    }
                    println!("{} — {} ({} steps)", t.id, t.name, t.steps.len());
                    for p in &t.params {
                        println!("  param {}", serde_json::to_string(p).unwrap_or_default());
                    }
                    0
                }
                Err(e) => {
                    let text = format!("{e:#}");
                    let code = if text.contains("not found") { Code::NotFound } else { Code::Usage };
                    fail(ctx, code, &text, None)
                }
            }
        }
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
        WorkspaceCmd::Node(NodeCmd::Claim(rest)) => rails_passthrough(ctx, &["wih", "pickup"], rest, false),
        WorkspaceCmd::Node(NodeCmd::Close(rest)) => rails_passthrough(ctx, &["wih", "close"], rest, false),
        WorkspaceCmd::Node(NodeCmd::Handoff(_)) => not_built(
            ctx,
            "workspace",
            "node handoff",
            "Close the node and let the next agent claim it.",
        ),
        WorkspaceCmd::Node(NodeCmd::List { dag, ready, mine }) => {
            if mine {
                return not_built(ctx, "workspace", "node list --mine", "Needs `agents whoami`; list without --mine.");
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
        WorkspaceCmd::Proof(_) => not_built(
            ctx,
            "workspace",
            "proof",
            "Proof is recorded through receipts and the judge today (workspace judge show).",
        ),
        WorkspaceCmd::Board(_) => not_built(ctx, "workspace", "board", "Use `workspace node list` for now."),
        WorkspaceCmd::Tasks(_) => not_built(ctx, "workspace", "tasks", "The cowork queue has not folded in yet."),
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
