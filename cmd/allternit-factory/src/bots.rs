//! `agents` verbs over teams and bots (SPEC §7 / §9): up, down (team),
//! whoami, snapshot, restore, pack, install, bot add, and the team/fields
//! enrichment of `agents ps --json`.
//!
//! Plans are pure ([`team_plan`]); acting on one is `team_apply::apply`.
//! Every mutation has `--dry-run`, which prints exactly the plan the real run
//! would act on and changes nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::agents::snapshot;
use allternit_factory_engine::agents::team::{self, LoadedTeam, TeamError};
use allternit_factory_engine::agents::team_apply::{
    self, ApiClient, ApiError, ApplyOptions, FactoryApi, StepOutcome, StepResult,
};
use allternit_factory_engine::agents::team_pack::{self, InstallOptions};
use allternit_factory_engine::agents::team_plan::{self, CurrentNode, LiveState};
use allternit_factory_engine::agents::whoami;
use allternit_factory_engine::agents::delivery;
use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::exec::{fail, ok_json, Code, Ctx};
use crate::work::read_events;

#[derive(Args)]
pub struct UpArgs {
    /// Team folder name under `.allternit/teams/`.
    pub team: String,
    /// Preset from team.yaml (default: its `default_preset`).
    #[arg(long)]
    pub preset: Option<String>,
    /// Computer to start terminal bots on (overrides each bot's `machine`).
    #[arg(long)]
    pub on: Option<String>,
    /// Directory terminal bots run in (default: the workspace root).
    #[arg(long)]
    pub workdir: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct SnapshotArgs {
    pub team: String,
    #[arg(long)]
    pub preset: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct RestoreArgs {
    pub team: String,
    /// Snapshot file (default: the team's latest).
    #[arg(long)]
    pub snapshot: Option<PathBuf>,
    /// Restore even though team.yaml changed since the snapshot.
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub workdir: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct PackArgs {
    pub team: String,
    /// Archive path (default: `<team>-<version>.team.tar.gz` in the current directory).
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct InstallArgs {
    /// A pack (`.tar.gz`), a team folder, or
    /// `https://github.com/<owner>/<repo>/tree/<40-hex commit>/<path>`.
    pub source: String,
    /// Replace an existing team of the same name.
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Subcommand)]
pub enum BotCmd {
    /// Create a bot (or rebind it) through allternit-api.
    Add(BotAddArgs),
}

#[derive(Args)]
pub struct BotAddArgs {
    pub slug: String,
    /// terminal | hosted | vendor
    #[arg(long)]
    pub binding: String,
    #[arg(long)]
    pub harness: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub role: Option<String>,
    #[arg(long)]
    pub machine: Option<String>,
    /// Pane slug of a running terminal bot.
    #[arg(long)]
    pub pane: Option<String>,
    #[arg(long)]
    pub vendor: Option<String>,
    /// official | channel | ui_bridge | local
    #[arg(long)]
    pub lane: Option<String>,
    /// hosted | linked | mirror
    #[arg(long)]
    pub mode: Option<String>,
    /// Slug (or id) of the bot that directs this vendor bot.
    #[arg(long)]
    pub directed_by: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
}

fn code_of(name: &str) -> Code {
    match name {
        "refused" => Code::Refused,
        "not_found" => Code::NotFound,
        "transport" => Code::Transport,
        "timeout" => Code::Timeout,
        "needs_person" => Code::NeedsPerson,
        "usage" => Code::Usage,
        _ => Code::Internal,
    }
}

fn team_fail(ctx: &Ctx, e: &TeamError) -> u8 {
    match e {
        TeamError::NotFound(t) => fail(
            ctx,
            Code::NotFound,
            &format!("team {t} not found (no .allternit/teams/{t}/team.yaml)"),
            Some("Create the team folder with a team.yaml, or install one with `agents install`."),
        ),
        TeamError::Io { .. } => fail(ctx, Code::Internal, &e.to_string(), None),
        _ => fail(ctx, Code::Usage, &e.to_string(), Some("Fix team.yaml (every problem is listed) and retry.")),
    }
}

fn load(ctx: &Ctx, name: &str) -> Result<LoadedTeam, u8> {
    team::load_team(&ctx.root_dir(), name).map_err(|e| team_fail(ctx, &e))
}

fn live(ctx: &Ctx, t: &LoadedTeam) -> Result<LiveState, u8> {
    team_apply::live_state(&ctx.root_dir(), t).map_err(|e| {
        fail(ctx, Code::Transport, &format!("{e:#}"), Some("Check the pane engine (allternit-factory pane status) and retry."))
    })
}

/// The API client, `Ok(None)` when not configured.
fn api(ctx: &Ctx) -> Result<Option<Arc<dyn FactoryApi>>, u8> {
    ApiClient::from_env()
        .map(|c| c.map(|c| Arc::new(c) as Arc<dyn FactoryApi>))
        .map_err(|e| fail(ctx, code_of(&e.code), &e.fact, Some(team_apply::API_ENV_ACTION)))
}

fn print_plan(plan: &[team_plan::TeamPlanStep]) {
    if plan.is_empty() {
        println!("nothing to do");
    }
    for s in plan {
        let action = serde_json::to_value(s.action).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        println!("{action:<5} {:<28} {}", s.agent, s.reason);
    }
}

/// Print results; exit with the worst failure code.
fn finish(ctx: &Ctx, doc: Value, results: &[StepResult]) -> u8 {
    let worst = team_apply::worst_code(results);
    if ctx.json {
        println!("{doc}");
    } else {
        for r in results {
            let mark = match r.outcome {
                StepOutcome::Ok => "ok  ",
                StepOutcome::Failed => "FAIL",
                StepOutcome::Skipped => "skip",
            };
            println!("{mark} {:<28} {}", r.step.agent, r.fact);
        }
    }
    match worst {
        None => 0,
        Some(code) => {
            let code = code_of(&code);
            if !ctx.json {
                eprintln!("error: some steps failed (see above)");
            }
            code.exit()
        }
    }
}

pub fn up(ctx: &Ctx, a: UpArgs) -> u8 {
    let t = match load(ctx, &a.team) {
        Ok(t) => t,
        Err(c) => return c,
    };
    let preset = match t.resolve_preset(a.preset.as_deref()) {
        Ok(p) => p,
        Err(e) => return team_fail(ctx, &e),
    };
    let live = match live(ctx, &t) {
        Ok(l) => l,
        Err(c) => return c,
    };
    let plan = match team_plan::plan_up(&t, preset.as_deref(), a.on.as_deref(), &live) {
        Ok(p) => p,
        Err(e) => return team_fail(ctx, &e),
    };
    let workdir = a.workdir.clone().unwrap_or_else(|| ctx.root_dir());
    if let Err(fact) = team_apply::check_workdirs(&t, preset.as_deref(), &plan, &workdir, &live) {
        return fail(ctx, Code::Refused, &fact, Some("Nothing was started. Pass --workdir, or change a bot's harness in a preset."));
    }
    if a.dry_run {
        if ctx.json {
            return ok_json(json!({ "plan": plan, "applied": false }));
        }
        println!("dry run: agents up {} (nothing was changed)", t.name);
        print_plan(&plan);
        return 0;
    }
    let api = match api(ctx) {
        Ok(a) => a,
        Err(c) => return c,
    };
    let opts = ApplyOptions { workdir: a.workdir, api, ..Default::default() };
    let results = team_apply::apply(&ctx.root_dir(), &t, preset.as_deref(), &plan, &opts);
    finish(ctx, json!({ "plan": plan, "applied": true, "results": results }), &results)
}

/// `agents down <team>` when a team of that name exists; `None` otherwise
/// (the caller stops the single pane).
pub fn down_team(ctx: &Ctx, target: &str, rm_worktree: bool, dry_run: bool) -> Option<u8> {
    if !team::valid_slug(target) || !team::team_dir(&ctx.root_dir(), target).join(team::TEAM_FILE).is_file() {
        return None;
    }
    let t = match load(ctx, target) {
        Ok(t) => t,
        Err(c) => return Some(c),
    };
    let live = match live(ctx, &t) {
        Ok(l) => l,
        Err(c) => return Some(c),
    };
    let plan = team_plan::plan_down(&t, &live);
    if dry_run {
        if ctx.json {
            return Some(ok_json(json!({ "plan": plan, "applied": false, "stopped": [] })));
        }
        println!("dry run: agents down {} (nothing was changed)", t.name);
        print_plan(&plan);
        return Some(0);
    }
    let opts = ApplyOptions { rm_worktree, ..Default::default() };
    let results = team_apply::apply(&ctx.root_dir(), &t, None, &plan, &opts);
    let stopped: Vec<&str> = results.iter().filter(|r| r.outcome == StepOutcome::Ok).map(|r| r.step.agent.as_str()).collect();
    Some(finish(ctx, json!({ "plan": plan, "applied": true, "stopped": stopped, "results": results }), &results))
}

pub fn whoami_cmd(ctx: &Ctx) -> u8 {
    let w = match whoami::whoami_from_env(|k| std::env::var(k).ok()) {
        Ok(w) => w,
        Err(e) => {
            return fail(
                ctx,
                Code::NotFound,
                &format!("not inside a factory pane: {e}"),
                Some("Run this inside a pane started by `agents up`."),
            )
        }
    };
    let events = match read_events(&ctx.root_dir()) {
        Ok(e) => e,
        Err(e) => return fail(ctx, Code::Internal, &format!("reading the ledger: {e:#}"), None),
    };
    let owned = whoami::owned_nodes(&events, &w.address);
    if ctx.json {
        return ok_json(json!({
            "bot": w.bot,
            "team": w.team,
            "address": w.address,
            "botId": w.bot_id,
            "paneId": w.pane_id,
            "wih": w.wih_id,
            "dag": w.dag_id,
            "ownedNodes": owned,
        }));
    }
    println!("{} (pane {})", w.address, w.pane_id.as_deref().unwrap_or("-"));
    for n in &owned {
        println!("  {}/{} {} [{}]", n.dag_id, n.node_id, n.title, n.status);
    }
    0
}

/// The node each live bot holds now (first open owned node that is in progress).
fn fill_current_nodes(ctx: &Ctx, t: &LoadedTeam, live: &mut LiveState) {
    let Ok(events) = read_events(&ctx.root_dir()) else { return };
    for b in &t.file.bots {
        let addr = team::address(&b.bot, &t.name);
        if let Some(n) = whoami::owned_nodes(&events, &addr).into_iter().find(|n| n.status == "IN_PROGRESS") {
            live.current_nodes.insert(addr, CurrentNode { dag_id: n.dag_id, node_id: n.node_id, title: n.title });
        }
    }
}

pub fn snapshot_cmd(ctx: &Ctx, a: SnapshotArgs) -> u8 {
    let t = match load(ctx, &a.team) {
        Ok(t) => t,
        Err(c) => return c,
    };
    let mut live = match live(ctx, &t) {
        Ok(l) => l,
        Err(c) => return c,
    };
    fill_current_nodes(ctx, &t, &mut live);
    if a.dry_run {
        return match snapshot::build_snapshot(&t, a.preset.as_deref(), &live, "(dry run: not taken)") {
            Ok(s) => ok_or_print(ctx, json!({ "dryRun": true, "snapshot": s })),
            Err(e) => fail(ctx, Code::Usage, &format!("{e:#}"), None),
        };
    }
    match snapshot::snapshot(&ctx.root_dir(), &t, a.preset.as_deref(), &live) {
        Ok(path) => ok_or_print(ctx, json!({ "path": path })),
        Err(e) => {
            let code = if e.downcast_ref::<TeamError>().is_some() { Code::Usage } else { Code::Internal };
            fail(ctx, code, &format!("{e:#}"), None)
        }
    }
}

fn ok_or_print(ctx: &Ctx, doc: Value) -> u8 {
    if ctx.json {
        ok_json(doc)
    } else {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        0
    }
}

pub fn restore(ctx: &Ctx, a: RestoreArgs) -> u8 {
    let t = match load(ctx, &a.team) {
        Ok(t) => t,
        Err(c) => return c,
    };
    let path = match a.snapshot.clone().or_else(|| snapshot::list_snapshots(&ctx.root_dir(), &t.name).pop()) {
        Some(p) => p,
        None => {
            return fail(
                ctx,
                Code::NotFound,
                &format!("team {} has no snapshot", t.name),
                Some("Take one with `agents snapshot <team>`."),
            )
        }
    };
    let snap = match snapshot::load_snapshot(&path) {
        Ok(s) => s,
        Err(e) => {
            let code = if path.exists() { Code::Usage } else { Code::NotFound };
            return fail(ctx, code, &format!("{e:#}"), None);
        }
    };
    if snap.team != t.name {
        return fail(ctx, Code::Usage, &format!("{} is a snapshot of team {}, not {}", path.display(), snap.team, t.name), None);
    }
    let changed = snap.team_changed(&t);
    let live = match live(ctx, &t) {
        Ok(l) => l,
        Err(c) => return c,
    };
    let plan = snapshot::restore_plan(&snap, &live);
    let workdir = a.workdir.clone().unwrap_or_else(|| ctx.root_dir());
    if let Err(fact) = team_apply::check_workdirs(&t, snap.preset.as_deref(), &plan, &workdir, &live) {
        return fail(ctx, Code::Refused, &fact, Some("Nothing was started. Pass --workdir."));
    }
    let base = json!({ "snapshot": path, "teamChanged": changed, "plan": plan });
    if a.dry_run {
        let mut doc = base;
        doc["applied"] = json!(false);
        return ok_or_print(ctx, doc);
    }
    if changed && !a.force {
        return fail(
            ctx,
            Code::Refused,
            &format!("team.yaml of {} changed since {} was taken; restoring could start bots the file no longer describes", t.name, path.display()),
            Some("Run `agents up <team>` for the current file, or restore with --force."),
        );
    }
    let api = match api(ctx) {
        Ok(a) => a,
        Err(c) => return c,
    };
    let opts = ApplyOptions { workdir: a.workdir, api, ..Default::default() };
    let results = team_apply::apply(&ctx.root_dir(), &t, snap.preset.as_deref(), &plan, &opts);
    let mut doc = base;
    doc["applied"] = json!(true);
    doc["results"] = json!(results);
    finish(ctx, doc, &results)
}

pub fn pack(ctx: &Ctx, a: PackArgs) -> u8 {
    let t = match load(ctx, &a.team) {
        Ok(t) => t,
        Err(c) => return c,
    };
    let version = t.file.version.clone().unwrap_or_else(|| "0.0.0".into());
    let out = a.out.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_default().join(format!("{}-{version}.team.tar.gz", t.name))
    });
    if a.dry_run {
        return ok_or_print(ctx, json!({ "dryRun": true, "team": t.name, "version": version, "archive": out }));
    }
    match team_pack::pack(&ctx.root_dir(), &t.name, &out) {
        Ok(r) => ok_or_print(ctx, serde_json::to_value(r).unwrap_or_default()),
        Err(e) => fail(ctx, Code::Usage, &format!("{e:#}"), None),
    }
}

pub fn install(ctx: &Ctx, a: InstallArgs) -> u8 {
    let opts = InstallOptions { dry_run: a.dry_run, force: a.force };
    let root = ctx.root_dir();
    let res = if a.source.starts_with("https://") || a.source.starts_with("http://") {
        team_pack::install_from_github(&root, &a.source, opts)
    } else {
        let p = Path::new(&a.source);
        if !p.exists() {
            return fail(ctx, Code::NotFound, &format!("{} does not exist", a.source), Some("Pass a .tar.gz pack, a team folder, or a pinned GitHub tree URL."));
        }
        team_pack::install_from_path(&root, p, opts)
    };
    match res {
        Ok(r) => ok_or_print(ctx, serde_json::to_value(r).unwrap_or_default()),
        Err(e) => {
            let text = format!("{e:#}");
            let (code, action) = if text.contains("already exists") {
                (Code::Refused, Some("Pass --force to replace it (its snapshots are kept)."))
            } else if text.contains("download of") || text.contains("run curl") {
                (Code::Transport, Some("Check the network and that the commit exists, then retry."))
            } else {
                (Code::Usage, Some("The pack was not installed; fix the problem named above."))
            };
            fail(ctx, code, &text, action)
        }
    }
}

fn api_fail(ctx: &Ctx, e: &ApiError) -> u8 {
    let action = match e.code.as_str() {
        "transport" | "timeout" => Some(team_apply::API_ENV_ACTION),
        _ => None,
    };
    fail(ctx, code_of(&e.code), &e.fact, action)
}

pub fn bot(ctx: &Ctx, cmd: BotCmd) -> u8 {
    let BotCmd::Add(a) = cmd;
    if !team::valid_slug(&a.slug) {
        return fail(ctx, Code::Usage, &format!("invalid bot slug {:?} (use a-z, 0-9, '-' and '_')", a.slug), None);
    }
    let binding = match team::Binding::parse(&a.binding) {
        Some(b) => b,
        None => return fail(ctx, Code::Usage, &format!("unknown binding {:?} (terminal | hosted | vendor)", a.binding), None),
    };
    let mut problems = vec![];
    match binding {
        team::Binding::Terminal if a.harness.is_none() => problems.push("a terminal bot needs --harness"),
        team::Binding::Vendor => {
            if a.vendor.is_none() {
                problems.push("a vendor bot needs --vendor");
            }
            if a.directed_by.is_none() {
                problems.push("a vendor bot needs --directed-by (the bot that directs it)");
            }
        }
        _ => {}
    }
    if let Some(l) = a.lane.as_deref().filter(|l| !team::VENDOR_LANES.contains(l)) {
        return fail(ctx, Code::Usage, &format!("unknown lane {l:?} ({})", team::VENDOR_LANES.join(" | ")), None);
    }
    if !problems.is_empty() {
        return fail(ctx, Code::Usage, &problems.join("; "), None);
    }
    let mut body = json!({
        "slug": a.slug,
        "name": a.name.clone().unwrap_or_else(|| a.slug.clone()),
        "role": a.role,
        "binding": match binding {
            team::Binding::Terminal => json!({ "type": "terminal", "harness": a.harness, "machine": a.machine, "paneId": a.pane }),
            team::Binding::Hosted => json!({ "type": "hosted" }),
            team::Binding::Vendor => json!({ "type": "vendor", "vendor": a.vendor, "lane": a.lane, "mode": a.mode, "directingBotId": a.directed_by }),
        },
    });
    if a.dry_run {
        return ok_or_print(ctx, json!({ "dryRun": true, "wouldPost": { "path": "/api/v1/factory/bots", "body": body }, "changed": false }));
    }
    let api = match api(ctx) {
        Ok(Some(api)) => api,
        Ok(None) => {
            return fail(
                ctx,
                Code::Transport,
                &format!("{}: bots are created through allternit-api", team_apply::API_NOT_SET_FACT),
                Some(team_apply::API_ENV_ACTION),
            )
        }
        Err(c) => return c,
    };
    if let Some(d) = a.directed_by.as_deref() {
        let bots = match api.list_bots() {
            Ok(b) => b,
            Err(e) => return api_fail(ctx, &e),
        };
        let id_of = |b: &Value| b["id"].as_str().or_else(|| b["bot"]["id"].as_str()).map(str::to_string);
        let slug_of = |b: &Value| b["slug"].as_str().or_else(|| b["bot"]["slug"].as_str()).map(str::to_string);
        let found = bots
            .iter()
            .find(|b| slug_of(b).as_deref() == Some(d))
            .and_then(id_of)
            .or_else(|| bots.iter().filter_map(id_of).find(|id| id == d));
        match found {
            Some(id) => body["binding"]["directingBotId"] = json!(id),
            None => return fail(ctx, Code::NotFound, &format!("no bot {d:?} to direct this vendor bot"), Some("Create the directing bot first.")),
        }
    }
    match api.upsert_bot(&body) {
        Ok(v) => ok_or_print(ctx, v),
        Err(e) => api_fail(ctx, &e),
    }
}

// ─── agents ps --json enrichment ───────────────────────────────────────────────

fn norm_harness(h: &str) -> &str {
    match h {
        "claude-code" => "claude",
        "gizzi-code" => "gizzi",
        "kimi-code" => "kimi",
        other => other,
    }
}

/// A team bot with a live pane: where it runs and how.
struct LiveBot {
    team: String,
    slug: String,
    address: String,
    role: String,
    harness: Option<String>,
    workdir: PathBuf,
    reach: Vec<String>,
}

fn live_team_bots(root: &Path) -> Vec<LiveBot> {
    let Ok(sessions) = team_apply::pane_sessions() else { return vec![] };
    let teams: Vec<LoadedTeam> = team::list_teams(root).iter().filter_map(|t| team::load_team(root, t).ok()).collect();
    let others: Vec<(String, Vec<String>)> =
        teams.iter().map(|t| (t.name.clone(), t.file.bots.iter().map(|b| b.bot.clone()).collect())).collect();
    let mut out = vec![];
    for t in &teams {
        let live = team_apply::live_state_from(t, &sessions, &others);
        let bots = t.effective_bots(None).unwrap_or_default();
        for (addr, wd) in &live.workdirs {
            let Some(b) = bots.iter().find(|b| &b.address == addr) else { continue };
            if b.binding != team::Binding::Terminal {
                continue;
            }
            let harness = delivery::load_record(wd, &b.slug).map(|r| r.harness).or(b.harness.clone());
            out.push(LiveBot {
                team: t.name.clone(),
                slug: b.slug.clone(),
                address: addr.clone(),
                role: b.role.clone(),
                harness,
                workdir: wd.clone(),
                reach: t.reach(&b.slug),
            });
        }
    }
    out
}

/// Fill team/address/role/reach and the delivered `fields` of `agents ps`
/// rows that map to exactly one live team bot (same workdir and harness).
/// Rows that don't map stay as they were (`fields` = `unavailable`); nothing
/// is guessed.
pub fn enrich_ps(mut doc: Value, root: &Path) -> Value {
    let bots = live_team_bots(root);
    let engine: Vec<Value> = doc["engine"]["agents"].as_array().cloned().unwrap_or_default();
    let Some(agents) = doc["agents"].as_array_mut() else { return doc };
    for a in agents.iter_mut() {
        let pane = a["id"].as_str().unwrap_or_default().to_string();
        let Some(src) = engine.iter().find(|e| e["paneId"].as_str() == Some(pane.as_str())) else { continue };
        let Some(cwd) = src["cwd"].as_str().map(PathBuf::from) else { continue };
        let harness = src["agent"].as_str().map(norm_harness);
        let matches: Vec<&LiveBot> = bots
            .iter()
            .filter(|b| b.workdir == cwd && b.harness.as_deref().map(norm_harness) == harness)
            .collect();
        if let [b] = matches.as_slice() {
            a["slug"] = json!(b.slug);
            a["team"] = json!(b.team);
            a["address"] = json!(b.address);
            a["role"] = json!(b.role);
            a["reach"] = json!(b.reach);
            a["pane"]["slug"] = json!(team_apply::pane_slug(&b.slug, &b.team));
            if let Some(f) = delivery::load_fields(&b.workdir, &b.slug) {
                a["fields"] = json!(f);
            }
        } else if let Some(name) = src["name"].as_str().filter(|n| team::valid_slug(n)) {
            if let Some(f) = delivery::load_fields(&cwd, name) {
                a["fields"] = json!(f);
            }
        }
    }
    doc
}
