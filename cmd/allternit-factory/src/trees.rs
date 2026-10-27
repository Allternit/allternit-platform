//! `agents tree …`: Agent Trees (one leased, seeded worktree per agent).
//! The rules live in the engine (`agents::trees`); this is the verb layer.

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use allternit_factory_engine::agents::trees::{
    checkpoint, land, ops, reaper, seed, LeaseStore, Owner, TreeError, TreeState, TreesConfig,
};

use crate::exec::{fail, ok_json, Code, Ctx};

#[derive(Subcommand)]
pub enum TreeCmd {
    /// Make (or reuse) a tree for a branch: fresh origin/main, disk gate,
    /// lease, seeded from the warm base, its own env.
    New(NewArgs),
    /// Every tree: owner, state, behind main, processes, overlaps, machine warnings.
    List {
        /// Include worktrees with no lease.
        #[arg(long)]
        all: bool,
        /// Measure disk use (du; slow on big trees).
        #[arg(long)]
        size: bool,
        /// Only the tree containing this path.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Print the tree's env (`eval "$(gizzi agents tree env)"`).
    Env { tree: Option<String> },
    /// Forward-only update to origin/main (clean trees already in main's past).
    Sync { tree: Option<String> },
    /// Save a checkpoint now (`--hook`: read a CLI hook payload on stdin).
    Checkpoint {
        tree: Option<String>,
        #[arg(long)]
        hook: bool,
        #[arg(long)]
        tool: Option<String>,
    },
    /// The tree's checkpoints, newest first.
    Checkpoints { tree: Option<String> },
    /// Put the files back to checkpoint `<n>` (the current state is saved first).
    Rewind {
        n: u32,
        tree: Option<String>,
        #[command(flatten)]
        who: Who,
    },
    /// Keep the tree for a later resume (pushes branch + checkpoints; never reaped).
    Pause {
        tree: Option<String>,
        #[command(flatten)]
        who: Who,
    },
    /// Make a paused tree active again.
    Resume {
        tree: Option<String>,
        #[command(flatten)]
        who: Who,
    },
    /// Land the tree: merge check, stop its processes, delete build output,
    /// branch and checkpoints, remove it, run the git-discipline check.
    Done {
        tree: Option<String>,
        /// Not merged: push it to a salvage/ branch, then remove it.
        #[arg(long)]
        abandon: bool,
        #[command(flatten)]
        who: Who,
    },
    /// Apply the merged + idle + clean rule to every worktree (dry run unless --apply).
    Reap {
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        size: bool,
    },
    /// Files changed in more than one live tree.
    Overlap,
    /// The warm base trees are cloned from.
    #[command(subcommand)]
    Base(BaseCmd),
    /// The scheduled reaper (launchd / systemd user timer).
    #[command(subcommand)]
    Reaper(ReaperCmd),
    /// PreToolUse hook: blocks `npm install` / `npm ci` in a pnpm tree.
    #[command(hide = true)]
    Guard {
        #[arg(long)]
        hook: bool,
    },
}

#[derive(Args)]
pub struct NewArgs {
    branch: String,
    #[arg(long)]
    path: Option<PathBuf>,
    /// `session:<id>`, `bot:<name>` or `human:<name>` (default: this terminal's bot, else you).
    #[arg(long)]
    owner: Option<String>,
    /// A one-off tree (a ledger commit): source only, expires in an hour.
    #[arg(long)]
    once: bool,
    /// Skip dependency installs (workers that only write code).
    #[arg(long)]
    source_only: bool,
    /// Don't clone from the base, check out plainly.
    #[arg(long)]
    plain: bool,
    /// Ignore the free-disk gate (a person decided).
    #[arg(long)]
    force_disk: bool,
    /// Use the last fetched origin/main when the fetch fails.
    #[arg(long)]
    offline: bool,
}

#[derive(Args)]
pub struct Who {
    /// Act on a tree you don't own (a person's override).
    #[arg(long)]
    as_owner: bool,
    #[arg(long)]
    owner: Option<String>,
}

#[derive(Subcommand)]
pub enum BaseCmd {
    /// Where the base is, its commit and how far behind main it is.
    Status,
    /// Make or fast-forward the base, install deps, and with --build warm it.
    Refresh {
        #[arg(long)]
        build: bool,
    },
}

#[derive(Subcommand)]
pub enum ReaperCmd {
    /// Run `reap --apply` every few hours.
    Install {
        #[arg(long, default_value_t = 6)]
        hours: u32,
    },
    Uninstall,
}

fn cwd(ctx: &Ctx) -> PathBuf {
    ctx.root_dir()
}

fn code_of(e: &TreeError) -> Code {
    match e.code {
        "usage" => Code::Usage,
        "refused" => Code::Refused,
        "not_found" => Code::NotFound,
        "transport" => Code::Transport,
        "needs_person" => Code::NeedsPerson,
        _ => Code::Internal,
    }
}

fn emit<T: serde::Serialize>(ctx: &Ctx, r: Result<T, TreeError>, human: impl FnOnce(&T)) -> u8 {
    match r {
        Ok(v) if ctx.json => ok_json(serde_json::to_value(&v).unwrap_or(Value::Null)),
        Ok(v) => {
            human(&v);
            0
        }
        Err(e) => fail(ctx, code_of(&e), &e.fact, Some(&e.action)),
    }
}

pub fn run(ctx: &Ctx, cmd: TreeCmd) -> u8 {
    let store = LeaseStore::open_default();
    let here = cwd(ctx);
    match cmd {
        TreeCmd::New(a) => {
            let r = ops::new_tree(
                &store,
                ops::NewRequest {
                    cwd: &here,
                    branch: &a.branch,
                    owner: Owner::resolve(a.owner.as_deref()),
                    path: a.path,
                    once: a.once,
                    source_only: a.source_only,
                    plain: a.plain,
                    force_disk: a.force_disk,
                    offline: a.offline,
                },
            );
            emit(ctx, r, |o| {
                let t = &o.tree;
                if o.reused {
                    println!("Reusing tree {} at {}", t.id, t.path.display());
                } else {
                    let seed = t.seed.as_ref().map(|s| format!("{:?}, {:.1} s", s.strategy, s.seconds).to_lowercase()).unwrap_or_default();
                    println!("Tree {} at {} ({seed})", t.id, t.path.display());
                }
                println!("  branch {} from origin/main {}", t.branch.as_deref().unwrap_or("-"), short_sha(&t.base));
                println!("  owner {}", t.owner.spec());
                println!("Next:\n  cd {}\n  eval \"$(gizzi agents tree env)\"", t.path.display());
            })
        }
        TreeCmd::List { all, size, path } => {
            let r = ops::list(&store, ops::ListRequest { repos: vec![here.clone()], path: path.as_deref(), all, with_size: size });
            emit(ctx, r, print_list)
        }
        TreeCmd::Env { tree } => {
            let r = ops::resolve(&store, tree.as_deref(), &here).map(|l| ops::env_lines(&l));
            match r {
                Ok(lines) if ctx.json => ok_json(json!({ "env": lines })),
                Ok(lines) => {
                    println!("{}", lines.join("\n"));
                    0
                }
                Err(e) => fail(ctx, code_of(&e), &e.fact, Some(&e.action)),
            }
        }
        TreeCmd::Sync { tree } => {
            let r = ops::resolve(&store, tree.as_deref(), &here).and_then(|l| ops::sync(&store, &l));
            emit(ctx, r, |t| println!("{} is at origin/main ({} behind)", t.id, t.behind_main.unwrap_or(0)))
        }
        TreeCmd::Checkpoint { tree, hook, tool } => {
            if hook {
                return checkpoint_hook(&store);
            }
            let r = ops::resolve(&store, tree.as_deref(), &here).and_then(|l| {
                let cp = checkpoint::save(&store, &l, tool.as_deref())?;
                ops::touch(&store, &l.id)?;
                Ok(cp)
            });
            emit(ctx, r, |cp| match cp {
                Some(cp) => println!("Saved checkpoint #{} ({} files)", cp.n, cp.files),
                None => println!("Nothing changed since the last checkpoint."),
            })
        }
        TreeCmd::Checkpoints { tree } => {
            let r = ops::resolve(&store, tree.as_deref(), &here).and_then(|l| checkpoint::list(&l).map_err(TreeError::from));
            emit(ctx, r.map(|c| json!({ "checkpoints": c })), |v| {
                let list = v["checkpoints"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    println!("No checkpoints yet (the CLI hooks save one after each edit).");
                }
                for c in list {
                    println!(
                        "#{:<4} {}  {:<8} {:>3} files  {}",
                        c["n"],
                        c["at"].as_str().unwrap_or(""),
                        c["tool"].as_str().unwrap_or("-"),
                        c["files"],
                        c["summary"].as_str().unwrap_or("")
                    );
                }
            })
        }
        TreeCmd::Rewind { n, tree, who } => {
            let caller = Owner::resolve(who.owner.as_deref());
            let r = ops::resolve(&store, tree.as_deref(), &here).and_then(|l| {
                ops::ensure_owner(&l, &caller, who.as_owner)?;
                Ok(json!({ "savedAs": checkpoint::rewind(&store, &l, n)? }))
            });
            emit(ctx, r, |v| println!("Rewound to #{n}; the previous state is checkpoint #{}", v["savedAs"]))
        }
        TreeCmd::Pause { tree, who } => set_state(ctx, &store, &here, tree, who, TreeState::Paused),
        TreeCmd::Resume { tree, who } => set_state(ctx, &store, &here, tree, who, TreeState::Active),
        TreeCmd::Done { tree, abandon, who } => {
            let caller = Owner::resolve(who.owner.as_deref());
            let r = resolve_or_adopt(&store, tree.as_deref(), &here, &caller).and_then(|l| {
                ops::ensure_owner(&l, &caller, who.as_owner)?;
                land::done(&store, &l, land::DoneRequest { abandon, skip_discipline: false })
            });
            emit(ctx, r, |d| {
                println!("Landed {} ({})", d.id, d.path.display());
                if let Some(s) = &d.salvaged_to {
                    println!("  work pushed to {s}");
                }
                if !d.stopped.is_empty() {
                    println!("  stopped {} process(es)", d.stopped.len());
                }
                if !d.build_dirs_removed.is_empty() {
                    println!("  deleted build output: {}", d.build_dirs_removed.join(", "));
                }
                println!(
                    "  branch {}: local {}, origin {}",
                    d.branch.as_deref().unwrap_or("-"),
                    if d.branch_deleted { "deleted" } else { "kept" },
                    if d.remote_branch_deleted { "deleted" } else { "not there / kept" }
                );
                if let Some(disc) = &d.discipline {
                    println!("git-discipline-check: {}\n{}", if disc.passed { "PASS" } else { "FAIL" }, disc.output);
                }
            })
        }
        TreeCmd::Reap { apply, size } => {
            let r = reaper::reap(&store, reaper::ReapRequest { repos: vec![here.clone()], apply, with_size: size });
            emit(ctx, r, print_reap)
        }
        TreeCmd::Overlap => {
            let r = ops::list(&store, ops::ListRequest { repos: vec![here.clone()], path: None, all: true, with_size: false });
            emit(ctx, r.map(|l| json!({ "overlaps": l.overlaps })), |v| {
                let list = v["overlaps"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    println!("No two live trees change the same file.");
                }
                for o in list {
                    println!("{} and {} both change: {}", o["a"].as_str().unwrap_or(""), o["b"].as_str().unwrap_or(""), join(&o["files"]));
                }
            })
        }
        TreeCmd::Base(BaseCmd::Status) => {
            let r = base_status(&here);
            emit(ctx, r, |v| {
                println!("base {}", v["path"].as_str().unwrap_or(""));
                if v["exists"].as_bool() == Some(true) {
                    println!("  at {} ({} behind origin/main), {}", v["head"].as_str().unwrap_or(""), v["behind"], if v["clean"].as_bool() == Some(true) { "clean" } else { "NOT clean: trees fall back to plain checkouts" });
                } else {
                    println!("  missing: `gizzi agents tree base refresh --build` makes it");
                }
            })
        }
        TreeCmd::Base(BaseCmd::Refresh { build }) => {
            let r = git_main(&here).and_then(|main| {
                let cfg = TreesConfig::load(&main)?;
                let head = seed::refresh_base(&main, &cfg, build)?;
                Ok(json!({ "path": seed::base_path(&main), "head": head, "built": build }))
            });
            emit(ctx, r, |v| println!("Base {} at {}", v["path"].as_str().unwrap_or(""), short_sha(v["head"].as_str().unwrap_or(""))))
        }
        TreeCmd::Reaper(rc) => {
            let (install, hours) = match rc {
                ReaperCmd::Install { hours } => (true, hours),
                ReaperCmd::Uninstall => (false, 0),
            };
            let engine = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("allternit-factory"));
            let r = reaper::schedule(&engine, &here, hours, install).map_err(TreeError::from);
            emit(ctx, r.map(|m| json!({ "result": m })), |v| println!("{}", v["result"].as_str().unwrap_or("")))
        }
        TreeCmd::Guard { hook } => {
            if hook {
                guard_hook()
            } else {
                fail(ctx, Code::Usage, "guard only runs as a hook", Some("It is registered by .steering/bin/steer-install.sh."))
            }
        }
    }
}

fn set_state(ctx: &Ctx, store: &LeaseStore, here: &Path, tree: Option<String>, who: Who, state: TreeState) -> u8 {
    let caller = Owner::resolve(who.owner.as_deref());
    let r = ops::resolve(store, tree.as_deref(), here).and_then(|l| {
        ops::ensure_owner(&l, &caller, who.as_owner)?;
        ops::set_state(store, &l, state)
    });
    emit(ctx, r, |t| println!("{} is {}", t.id, if t.state == TreeState::Paused { "paused (kept for a resume; branch and checkpoints pushed)" } else { "active" }))
}

/// `done` from inside a worktree made by hand: lease it to the caller first.
fn resolve_or_adopt(store: &LeaseStore, tree: Option<&str>, here: &Path, caller: &Owner) -> Result<allternit_factory_engine::agents::trees::Lease, TreeError> {
    match ops::resolve(store, tree, here) {
        Ok(l) => Ok(l),
        Err(e) if e.code == "not_found" && tree.is_none() => {
            let main = git_main(here)?;
            let top = allternit_factory_engine::agents::trees::git::toplevel(here).map_err(TreeError::from)?;
            ops::adopt(store, &main, &top, caller.clone())
        }
        Err(e) => Err(e),
    }
}

fn git_main(here: &Path) -> Result<PathBuf, TreeError> {
    allternit_factory_engine::agents::trees::git::main_checkout(here)
        .map_err(|e| TreeError::usage(format!("{e:#}"), "Run it from inside the repo (or pass --root)."))
}

fn base_status(here: &Path) -> Result<Value, TreeError> {
    use allternit_factory_engine::agents::trees::git;
    let main = git_main(here)?;
    let path = seed::base_path(&main);
    if !path.join(".git").exists() {
        return Ok(json!({ "path": path, "exists": false }));
    }
    let head = git::rev(&path, "HEAD").unwrap_or_default();
    let behind = git::behind_ahead(&path, "origin/main").map(|(b, _)| b).unwrap_or(0);
    let clean = git::dirty_files(&path).map(|d| d.is_empty()).unwrap_or(false);
    Ok(json!({ "path": path, "exists": true, "head": short_sha(&head), "behind": behind, "clean": clean }))
}

fn short_sha(s: &str) -> String {
    s.chars().take(9).collect()
}

fn join(v: &Value) -> String {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

fn print_list(l: &ops::TreeList) {
    if l.trees.is_empty() {
        println!("No trees. Start one with `gizzi agents tree new <branch>`.");
    }
    for t in &l.trees {
        let state = if !t.leased { "no lease".to_string() } else { format!("{:?}", t.state).to_lowercase() };
        println!("{}  [{state}]  {}", t.id, t.path.display());
        let mut facts = vec![format!("owner {}", t.owner.label)];
        if let Some(b) = t.behind_main.filter(|b| *b > 0) {
            facts.push(format!("{b} behind main"));
        }
        if let Some(d) = t.dirty_files.filter(|d| *d > 0) {
            facts.push(format!("{d} uncommitted"));
        }
        if let Some(s) = t.size_bytes {
            facts.push(gb(s));
        }
        if t.checkpoint_count > 0 {
            facts.push(format!("{} checkpoints", t.checkpoint_count));
        }
        println!("    {}", facts.join(" · "));
        for p in &t.processes {
            let tag = if p.protected { "agent" } else if p.dev_server { "dev server" } else { "process" };
            println!("    {tag} {} {}", p.pid, p.command.chars().take(80).collect::<String>());
        }
    }
    for o in &l.overlaps {
        println!("! {} and {} both change: {}", o.a, o.b, o.files.join(", "));
    }
    if let Some(f) = l.machine.free_bytes {
        println!("Disk: {} free", gb(f));
    }
    for w in &l.machine.warnings {
        println!("! {w}");
    }
}

fn print_reap(r: &reaper::ReapReport) {
    let verb = if r.applied { "" } else { "would be " };
    let size = |s: Option<u64>| s.map(|b| format!(" ({})", gb(b))).unwrap_or_default();
    for x in &r.removed {
        println!("{verb}removed  {}{}  — {}", x.path.display(), size(x.size_bytes), x.reason);
    }
    for x in &r.cleaned {
        println!("{verb}cleaned  {}{}  — {}", x.path.display(), size(x.size_bytes), x.reason);
    }
    for s in &r.stopped {
        println!("{verb}stopped  pid {} ({} h) in {}: {}", s.pid, s.age_hours, s.path.display(), s.command.chars().take(60).collect::<String>());
    }
    for x in &r.kept {
        println!("kept     {}{}  — {}", x.path.display(), size(x.size_bytes), x.reason);
    }
    if let (Some(a), Some(b)) = (r.free_bytes_before, r.free_bytes_after) {
        println!("Disk: {} free before, {} after", gb(a), gb(b));
    }
    if !r.applied {
        println!("Dry run. `gizzi agents tree reap --apply` does it.");
    }
}

fn read_payload() -> Value {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

/// PostToolUse hook (Claude Code, codex, kimi, gizzi): checkpoint the tree
/// the tool ran in. Silent and exit 0 outside a tree or on any failure, so a
/// hook never blocks an agent.
fn checkpoint_hook(store: &LeaseStore) -> u8 {
    let p = read_payload();
    let cwd = p["cwd"].as_str().map(PathBuf::from).or_else(|| std::env::current_dir().ok());
    let Some(cwd) = cwd else { return 0 };
    let tool = p["tool_name"].as_str().or_else(|| p["tool"].as_str());
    // Edits only: reads and searches don't change files.
    if let Some(t) = tool {
        if !matches!(t, "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Bash" | "shell" | "apply_patch" | "edit" | "write") {
            return 0;
        }
    }
    let Ok(Some(lease)) = store.find_by_path(&cwd) else { return 0 };
    if lease.state != TreeState::Active {
        return 0;
    }
    if checkpoint::save(store, &lease, tool).is_ok() {
        let _ = ops::touch(store, &lease.id);
    }
    0
}

/// PreToolUse hook: `npm install` / `npm ci` in a git worktree of a pnpm repo
/// duplicates what the pnpm store hardlinks (AGENTS.md "pnpm only").
fn guard_hook() -> u8 {
    let p = read_payload();
    let ti = if p["tool_input"].is_object() { &p["tool_input"] } else { &p["input"] };
    let command = ti["command"].as_str().unwrap_or("");
    if !npm_install(command) {
        return 0;
    }
    let cwd = p["cwd"].as_str().map(PathBuf::from).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    let top = allternit_factory_engine::agents::trees::git::toplevel(&cwd);
    let Ok(top) = top else { return 0 };
    if !top.join("pnpm-lock.yaml").exists() {
        return 0;
    }
    let reason = format!(
        "[agent trees] BLOCKED: `{command}` in {} — this repo uses pnpm, and npm would copy what the pnpm store hardlinks. Use `pnpm install --frozen-lockfile --offline` (or `gizzi agents tree new`, which does it).",
        top.display()
    );
    println!("{}", json!({ "decision": "block", "reason": reason }));
    eprintln!("{reason}");
    2
}

fn npm_install(command: &str) -> bool {
    command.split(['&', ';', '|']).any(|seg| {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        let Some(i) = toks.iter().position(|t| *t == "npm" || t.ends_with("/npm")) else { return false };
        let rest = &toks[i + 1..];
        let verb = rest.iter().find(|t| !t.starts_with('-'));
        matches!(verb, Some(&"install") | Some(&"i") | Some(&"ci") | Some(&"add"))
            && !rest.iter().any(|t| *t == "-g" || *t == "--global")
    })
}

#[cfg(test)]
mod tests {
    use super::npm_install;

    #[test]
    fn npm_install_detection() {
        assert!(npm_install("npm install"));
        assert!(npm_install("cd x && npm ci"));
        assert!(npm_install("npm i left-pad"));
        assert!(!npm_install("npm install -g pnpm"));
        assert!(!npm_install("pnpm install"));
        assert!(!npm_install("npm run build"));
    }
}
