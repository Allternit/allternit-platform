//! Part verbs implemented by the engine itself (not passed through to the
//! maintenance CLI or the pane CLI): `orchestration send|capture|transcript|drain`
//! and `agents ps|down|recover|wall`.

use std::path::PathBuf;

use serde_json::json;

use allternit_factory_engine::registry::Registry;
use allternit_factory_engine::send::{self as send_mod, ApiLink, SendCtx, SendRequest};
use allternit_factory_engine::view;
use allternit_factory_engine::agents::team as team_mod;
use allternit_factory_engine::agents::team_apply::pane_slug_for_address;
use allternit_factory_engine::backend;
use allternit_factory_engine::registry::{self as registry_mod, Entry};
use allternit_factory_engine::spawn::{caller_identity, RecoverOptions, Spawner};

use crate::exec::{fail, ok_json, Code, Ctx};

fn runtime(ctx: &Ctx) -> Result<tokio::runtime::Runtime, u8> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(ctx, Code::Internal, &format!("async runtime: {e}"), None))
}

/// Who is sending from the command line.
fn cli_sender() -> String {
    for var in ["ALLTERNIT_FACTORY_LEAD", "USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return format!("user:{v}");
            }
        }
    }
    "user:human".to_string()
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

#[allow(clippy::too_many_arguments)]
pub fn send(
    ctx: &Ctx,
    to: String,
    text: String,
    queue: bool,
    thread: Option<String>,
    node: Option<String>,
    dag: Option<String>,
    key: Option<String>,
    dry_run: bool,
) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let sctx = SendCtx {
        root: ctx.root_dir(),
        registry: Registry::open_default(),
        api: ApiLink::from_env(),
        sender: cli_sender(),
    };
    let req = SendRequest {
        to,
        text,
        queue,
        thread_id: thread,
        node_id: node,
        dag_id: dag,
        idempotency_key: key,
        dry_run,
    };
    if dry_run {
        return match rt.block_on(send_mod::plan(&sctx, &req)) {
            Ok(plan) => {
                if ctx.json {
                    return ok_json(json!({ "dryRun": true, "plan": plan }));
                }
                println!("would send to {} via {} (recorded on {}: {})", plan.to, plan.via, plan.thread_id, plan.records.join(", "));
                0
            }
            Err(e) => fail(ctx, code_of(e.code), &e.fact, Some(&e.action)),
        };
    }
    match rt.block_on(send_mod::send(&sctx, &req)) {
        Ok(d) if d.state == "failed" => {
            let fact = format!("not delivered to {} via {}: {}", d.to, d.via, d.detail.clone().unwrap_or_default());
            let action = "The message is recorded; fix the cause and send again.";
            if ctx.json {
                println!(
                    "{}",
                    json!({ "error": { "code": "transport", "fact": fact, "action": action }, "delivery": d })
                );
            } else {
                eprintln!("error: {fact}\n  {action}");
            }
            Code::Transport.exit()
        }
        Ok(d) => {
            if ctx.json {
                return ok_json(serde_json::to_value(&d).unwrap_or_default());
            }
            let mut line = format!("{} via {} to {}", d.state, d.via, d.to);
            if let Some(t) = &d.ticket {
                line.push_str(&format!(" ({t})"));
            }
            if let Some(detail) = &d.detail {
                line.push_str(&format!(" — {detail}"));
            }
            println!("{line}");
            0
        }
        Err(e) => fail(ctx, code_of(e.code), &e.fact, Some(&e.action)),
    }
}

pub fn ps(ctx: &Ctx, cwd: Option<String>) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let snap = match rt.block_on(view::snapshot(&ctx.root_dir(), &Registry::open_default(), cwd.as_deref())) {
        Ok(s) => s,
        Err(e) => {
            let code = if allternit_factory_engine::backend::is_transport(&e) { Code::Transport } else { Code::Internal };
            return fail(ctx, code, &format!("{e:#}"), None);
        }
    };
    // Team bots' panes get their team/address/role/reach and delivered fields.
    let agents = crate::bots::enrich_ps(json!(snap.agents), &ctx.root_dir());
    if ctx.json {
        return ok_json(json!({ "agents": agents, "engine": snap.engine }));
    }
    if let Some(err) = &snap.engine.error {
        eprintln!("pane engine: {err}");
    }
    for change in &snap.reconciled {
        eprintln!("reconciled {}: {}", change.session, change.kind);
    }
    let rows = agents.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("no agents");
    }
    for a in &rows {
        println!(
            "{:<24} {:<9} {:<10} {}",
            a["address"].as_str().unwrap_or("-"),
            a["state"].as_str().unwrap_or("-"),
            a["binding"]["harness"].as_str().unwrap_or("-"),
            a["pane"]["id"].as_str().unwrap_or("-")
        );
    }
    0
}

// ─── Agent sessions: down, recover, capture, transcript, drain ─────────────────
//
// All on the engine path: the session registry, the [`Spawner`] (the one
// spawn path, so the spawn gate) and the installed pane backend.
//
// [`Spawner`]: allternit_factory_engine::spawn::Spawner


/// The session of a local agent: an id, address (`bot@team`) or slug that
/// `agents ps` lists, else a session the registry still records.
fn local_session(ctx: &Ctx, rt: &tokio::runtime::Runtime, to: &str) -> Result<(String, Option<Entry>), u8> {
    let registry = Registry::open_default();
    let agents = rt.block_on(send_mod::local_agents(&ctx.root_dir(), &registry)).unwrap_or_default();
    let slug = match view::find(&agents, to) {
        Some(a) => a.slug.clone(),
        None => pane_slug_for_address(registry_mod::slug_of(to)),
    };
    let session = registry_mod::session_of(&slug);
    let entry = registry.load().ok().and_then(|f| f.sessions.get(&session).cloned());
    let live = backend::backend().ok().and_then(|b| b.find(&session).ok().flatten()).is_some();
    if entry.is_none() && !live {
        return Err(fail(
            ctx,
            Code::NotFound,
            &format!("no agent session {to} ({session}) on this computer"),
            Some("List agents with `gizzi agents ps`."),
        ));
    }
    Ok((session, entry))
}

fn engine_err(ctx: &Ctx, e: &anyhow::Error) -> u8 {
    let code = if backend::is_transport(e) { Code::Transport } else { Code::Internal };
    fail(ctx, code, &format!("{e:#}"), None)
}

/// `agents down <slug>` for one session: close its pane, mark the record
/// dead, and with `--rm-worktree` remove the worktree it was started in.
pub fn down(ctx: &Ctx, to: String, rm_worktree: bool, dry_run: bool) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let (session, _) = match local_session(ctx, &rt, &to) {
        Ok(s) => s,
        Err(code) => return code,
    };
    if dry_run {
        if ctx.json {
            return ok_json(json!({ "dryRun": true, "wouldStop": session, "rmWorktree": rm_worktree, "changed": false }));
        }
        println!("dry run: would stop {session}{}", if rm_worktree { " and remove its worktree" } else { "" });
        println!("nothing was changed");
        return 0;
    }
    let spawner = match Spawner::new(ctx.root_dir()) {
        Ok(s) => s,
        Err(e) => return engine_err(ctx, &e),
    };
    if let Err(e) = rt.block_on(spawner.kill(registry_mod::slug_of(&session), rm_worktree)) {
        return engine_err(ctx, &e);
    }
    if ctx.json {
        return ok_json(json!({ "stopped": session, "rmWorktree": rm_worktree }));
    }
    println!("stopped {session}");
    0
}

/// `agents recover`: the plan (default) or the respawns (`--apply`).
pub fn recover(ctx: &Ctx, slug: Option<String>, apply: bool, lead: Option<String>, as_human: bool) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let spawner = match Spawner::new(ctx.root_dir()) {
        Ok(s) => s,
        Err(e) => return engine_err(ctx, &e),
    };
    let caller = caller_identity(lead.as_deref());
    let slug = slug.map(|s| pane_slug_for_address(registry_mod::slug_of(&s)));
    let steps = match rt.block_on(spawner.recover(RecoverOptions { only: slug.as_deref(), apply, caller: &caller, as_human })) {
        Ok(s) => s,
        Err(e) => return engine_err(ctx, &e),
    };
    let bad = steps.iter().any(|s| s.action == "refuse" || s.action == "failed");
    if ctx.json {
        println!("{}", json!({ "applied": apply, "steps": steps }));
        return if bad { Code::Refused.exit() } else { 0 };
    }
    let planned = steps.iter().filter(|s| s.action == "plan").count();
    for s in &steps {
        let cmd = s.argv.as_ref().map(|a| format!(" cmd={}", a.join(" "))).unwrap_or_default();
        println!("{} {}: {}{cmd}", s.action.to_uppercase(), s.session, s.detail);
    }
    if steps.is_empty() {
        println!("nothing to recover");
    } else if planned > 0 && !apply {
        println!("dry run: rerun with --apply to respawn");
    }
    if bad {
        Code::Refused.exit()
    } else {
        0
    }
}

/// `agents model <bot@team> <model> [--restart] [--clear] [--dry-run]`: set a
/// Terminal bot's model. It is recorded in the team's `overrides.json` and
/// applies on the bot's next start; `--restart` also relaunches a running
/// bot now with its harness's resume (the conversation continues).
pub fn model(ctx: &Ctx, args: &[String]) -> u8 {
    let (mut restart, mut clear, mut dry_run) = (false, false, false);
    let mut pos: Vec<&str> = vec![];
    for a in args {
        match a.as_str() {
            "--restart" => restart = true,
            "--clear" => clear = true,
            "--dry-run" => dry_run = true,
            "--json" => {}
            _ if a.starts_with("--") => return fail(ctx, Code::Usage, &format!("unknown option {a}"), None),
            _ => pos.push(a),
        }
    }
    let usage = "allternit-factory agents model <bot@team> <model> [--restart] [--dry-run]  (or <bot@team> --clear)";
    let (address, model) = match (pos.as_slice(), clear) {
        ([a], true) => (*a, None),
        ([a, m], false) => (*a, Some(*m)),
        _ => return fail(ctx, Code::Usage, "agents model needs <bot@team> and a model (or --clear)", Some(usage)),
    };
    let Some((bot, team)) = address.split_once('@') else {
        return fail(
            ctx,
            Code::Usage,
            &format!("{address} is not a team bot address (bot@team)"),
            Some("Hosted and vendor bots keep their model on your account: use `gizzi agents bot` or the app."),
        );
    };
    let root = ctx.root_dir();
    let loaded = match team_mod::load_team(&root, team) {
        Ok(t) => t,
        Err(e) => {
            let code = if matches!(e, team_mod::TeamError::NotFound(_)) { Code::NotFound } else { Code::Usage };
            return fail(ctx, code, &e.to_string(), None);
        }
    };
    let Some(row) = loaded.file.bots.iter().find(|b| b.bot == bot) else {
        return fail(ctx, Code::NotFound, &format!("{bot} is not on team {team}"), Some("List the team's bots with `gizzi agents ps`."));
    };
    let before = row.model.clone();
    let session = registry_mod::session_of(&pane_slug_for_address(address));
    let live = backend::backend().ok().and_then(|b| b.find(&session).ok().flatten()).is_some();
    let will_restart = restart && live;
    if dry_run {
        let plan = json!({ "bot": address, "from": before, "to": model, "restart": will_restart, "session": session });
        if ctx.json {
            return ok_json(json!({ "dryRun": true, "plan": plan }));
        }
        println!(
            "would set {address} model {} -> {}{}",
            before.as_deref().unwrap_or("(harness default)"),
            model.unwrap_or("(team.yaml)"),
            if will_restart { ", then restart it with resume" } else if live { " (applies on its next start)" } else { "" }
        );
        return 0;
    }
    if let Err(e) = team_mod::set_bot_model(&root, team, bot, model) {
        return fail(ctx, Code::Internal, &e.to_string(), None);
    }
    let mut restarted = false;
    if will_restart {
        let rt = match runtime(ctx) {
            Ok(rt) => rt,
            Err(code) => return code,
        };
        // The relaunch reuses the recorded argv, so swap its --model first.
        let registry = Registry::open_default();
        let new_model = model.map(str::to_string);
        let _ = registry.update(|f| {
            if let Some(e) = f.sessions.get_mut(&session) {
                if let Some(argv) = e.argv.as_mut() {
                    set_model_flag(argv, new_model.as_deref());
                }
            }
        });
        let spawner = match Spawner::new(root.clone()) {
            Ok(s) => s,
            Err(e) => return engine_err(ctx, &e),
        };
        if let Err(e) = rt.block_on(spawner.kill(registry_mod::slug_of(&session), false)) {
            return engine_err(ctx, &e);
        }
        let caller = caller_identity(None);
        let slug = registry_mod::slug_of(&session).to_string();
        match rt.block_on(spawner.recover(RecoverOptions { only: Some(&slug), apply: true, caller: &caller, as_human: true })) {
            Ok(steps) => {
                if let Some(f) = steps.iter().find(|s| s.action == "failed" || s.action == "refuse") {
                    return fail(ctx, Code::Refused, &format!("model saved, but restarting {session} failed: {}", f.detail), Some("It applies on the next start; or run `gizzi agents recover --apply`."));
                }
                restarted = steps.iter().any(|s| s.action == "recovered")
            }
            Err(e) => return engine_err(ctx, &e),
        }
    }
    let applies = if restarted { "now" } else { "next start" };
    if ctx.json {
        return ok_json(json!({ "bot": address, "from": before, "to": model, "applies": applies, "restarted": restarted }));
    }
    println!("{address}: model {} (applies {applies})", model.unwrap_or("from team.yaml"));
    0
}

/// Set (or drop) `--model <m>` in a harness argv.
fn set_model_flag(argv: &mut Vec<String>, model: Option<&str>) {
    if let Some(i) = argv.iter().position(|a| a == "--model") {
        argv.drain(i..(i + 2).min(argv.len()));
    }
    argv.retain(|a| !a.starts_with("--model="));
    if let Some(m) = model {
        let at = if argv.is_empty() { 0 } else { 1 };
        argv.insert(at, m.to_string());
        argv.insert(at, "--model".to_string());
    }
}

/// `agents handoff <bot@team> [--note <text>] [--lines N] [--dry-run]`: hand
/// a Terminal bot's seat to a fresh session (its context is full, or it's
/// stuck). Saves the pane's last screen and the note to a handoff file,
/// stops the old session (recover won't bring it back), starts the bot
/// fresh, and tells the new session to read the handoff first.
pub fn handoff(ctx: &Ctx, args: &[String]) -> u8 {
    let mut note: Option<String> = None;
    let mut lines: u32 = 200;
    let mut dry_run = false;
    let mut pos: Vec<&str> = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--json" => {}
            "--note" => match it.next() {
                Some(v) => note = Some(v.clone()),
                None => return fail(ctx, Code::Usage, "--note needs text", None),
            },
            "--lines" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => lines = n,
                None => return fail(ctx, Code::Usage, "--lines needs a number", None),
            },
            _ if a.starts_with("--") => return fail(ctx, Code::Usage, &format!("unknown option {a}"), None),
            _ => pos.push(a),
        }
    }
    let [address] = pos.as_slice() else {
        return fail(ctx, Code::Usage, "agents handoff needs one bot address (bot@team)", Some("allternit-factory agents handoff <bot@team> [--note <text>]"));
    };
    let Some((bot, team)) = address.split_once('@') else {
        return fail(
            ctx,
            Code::Usage,
            &format!("{address} is not a team bot address (bot@team)"),
            Some("A hosted bot's thread hands off in the app (a new window seeded with a checkpoint)."),
        );
    };
    let root = ctx.root_dir();
    if let Err(e) = team_mod::load_team(&root, team).and_then(|t| {
        if t.file.bots.iter().any(|b| b.bot == bot) { Ok(()) } else { Err(team_mod::TeamError::NotFound(format!("{bot}@{team}"))) }
    }) {
        return fail(ctx, Code::NotFound, &e.to_string(), Some("List the team's bots with `gizzi agents ps`."));
    }
    let session = registry_mod::session_of(&pane_slug_for_address(address));
    let backend = match backend::backend() {
        Ok(b) => b,
        Err(e) => return engine_err(ctx, &e),
    };
    let screen = match backend.capture(&session, lines) {
        Ok(t) => t,
        Err(e) => {
            let code = if backend::is_transport(&e) { Code::Transport } else { Code::NotFound };
            return fail(ctx, code, &format!("{address} is not running ({e:#})"), Some("Start it with `gizzi agents up`; there's nothing to hand off."));
        }
    };
    let entry = Registry::open_default().load().ok().and_then(|f| f.sessions.get(&session).cloned());
    let workdir = entry.as_ref().map(|e| PathBuf::from(&e.cwd)).filter(|p| p.is_dir());
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string();
    let note_path = root.join(".allternit/factory/handoffs").join(format!("{bot}-{team}-{stamp}.md"));
    if dry_run {
        let plan = json!({ "from": session, "note": note_path.display().to_string(), "lines": lines, "workdir": workdir.as_ref().map(|p| p.display().to_string()) });
        if ctx.json {
            return ok_json(json!({ "dryRun": true, "plan": plan }));
        }
        println!("would save {address}'s last {lines} lines to {}, stop {session}, start it fresh and point it at the note", note_path.display());
        return 0;
    }
    let body = format!(
        "# Handoff: {address}\n\nA fresh session takes over from `{session}` ({stamp}).\n\n## Note\n\n{}\n\n## Where it left off (last {lines} lines of the pane)\n\n```\n{}\n```\n",
        note.as_deref().unwrap_or("(none)"),
        screen.trim_end()
    );
    if let Err(e) = std::fs::create_dir_all(note_path.parent().unwrap()).and_then(|_| std::fs::write(&note_path, body)) {
        return fail(ctx, Code::Internal, &format!("writing {}: {e}", note_path.display()), None);
    }
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let spawner = match Spawner::new(root.clone()) {
        Ok(s) => s,
        Err(e) => return engine_err(ctx, &e),
    };
    if let Err(e) = rt.block_on(spawner.kill(registry_mod::slug_of(&session), false)) {
        return engine_err(ctx, &e);
    }
    // Handed off, not crashed: `agents recover` must not relaunch it.
    let _ = Registry::open_default().update(|f| {
        if let Some(e) = f.sessions.get_mut(&session) {
            e.lifecycle = Some("finished".to_string());
        }
    });
    let started = match crate::bots::spawn_one(ctx, team, address, workdir) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let text = format!(
        "You are taking over as {address} from a previous session. Read {} first: it has the note and where that session left off. Then continue the work.",
        note_path.display()
    );
    let sctx = SendCtx { root: root.clone(), registry: Registry::open_default(), api: ApiLink::from_env(), sender: cli_sender() };
    let req = SendRequest { to: address.to_string(), text, queue: false, thread_id: None, node_id: None, dag_id: None, idempotency_key: None, dry_run: false };
    let mut delivery = rt.block_on(send_mod::send(&sctx, &req)).map(|d| json!(d)).unwrap_or_else(|e| json!({ "state": "failed", "detail": e.fact }));
    // A fresh pane is often still starting, so the paste lands in its
    // mailbox. Drain it (no duplicate sends) until it's delivered.
    if delivery["state"] == "queued" {
        for _ in 0..15 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            match backend::drain(&root, &session, true) {
                Ok(d) if !d.is_empty() && d.iter().all(|m| m.delivered) => {
                    delivery["state"] = json!("verified");
                    delivery["detail"] = json!("delivered from the mailbox once the fresh session was ready");
                    break;
                }
                _ => {}
            }
        }
    }
    if ctx.json {
        return ok_json(json!({ "handedOff": session, "note": note_path.display().to_string(), "started": started, "delivery": delivery }));
    }
    println!("{address}: handed off to a fresh session; note at {}", note_path.display());
    if delivery["state"] != "verified" {
        println!("the first message is waiting in its mailbox; deliver it with `gizzi orchestration drain {address}`");
    }
    0
}

/// Send without printing (a follow-up inside another verb's output).
pub(crate) fn send_quiet(ctx: &Ctx, to: &str, text: String, node: Option<String>, dag: Option<String>) -> Option<serde_json::Value> {
    let rt = runtime(ctx).ok()?;
    let sctx = SendCtx { root: ctx.root_dir(), registry: Registry::open_default(), api: ApiLink::from_env(), sender: cli_sender() };
    let req = SendRequest { to: to.to_string(), text, queue: false, thread_id: None, node_id: node, dag_id: dag, idempotency_key: None, dry_run: false };
    rt.block_on(send_mod::send(&sctx, &req)).ok().map(|d| json!(d))
}

/// `orchestration capture <to> [lines]`: the last lines of the agent's pane.
pub fn capture(ctx: &Ctx, to: String, lines: Option<u32>) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let (session, _) = match local_session(ctx, &rt, &to) {
        Ok(s) => s,
        Err(code) => return code,
    };
    drop(rt);
    let lines = lines.unwrap_or(25);
    let text = match backend::backend().and_then(|b| b.capture(&session, lines)) {
        Ok(t) => t,
        Err(e) => {
            let code = if backend::is_transport(&e) { Code::Transport } else { Code::NotFound };
            return fail(ctx, code, &format!("{e:#}"), Some("The pane is not live; read its transcript with `orchestration transcript`."));
        }
    };
    if ctx.json {
        return ok_json(json!({ "session": session, "lines": lines, "text": text }));
    }
    print!("{text}");
    0
}

/// The `ao-<slug>` sessions of a team's bots: registry entries that record
/// the team, plus the team.yaml bots' pane slugs (a pane started before the
/// registry recorded teams).
fn team_sessions(ctx: &Ctx, team: &str) -> Vec<String> {
    let mut sessions: Vec<String> = Registry::open_default()
        .load()
        .map(|file| {
            file.sessions
                .into_iter()
                .filter(|(_, e)| !e.dead && e.bot.as_ref().and_then(|b| b.team.as_deref()) == Some(team))
                .map(|(session, _)| session)
                .collect()
        })
        .unwrap_or_default();
    if let Ok(loaded) = team_mod::load_team(&ctx.root_dir(), team) {
        for b in &loaded.file.bots {
            let session = allternit_factory_engine::spawn::session_name(&allternit_factory_engine::agents::team_apply::pane_slug(&b.bot, team));
            if !sessions.contains(&session) {
                sessions.push(session);
            }
        }
    }
    sessions
}

/// `agents wall <team>`: the live wall with its Agents panel narrowed to the
/// team's bots (titled with the team), focused on the first. The view is
/// removed when the wall is closed.
pub fn wall(ctx: &Ctx, team: &str) -> u8 {
    if ctx.json {
        return fail(ctx, Code::Usage, "agents wall is interactive and has no JSON form", None);
    }
    let sessions = team_sessions(ctx, team);
    let live = match allternit_factory_pane::factory_backend::set_wall_view(team, &sessions) {
        Ok(n) => n,
        Err(e) => {
            let code = if backend::is_transport(&e) { Code::Transport } else { Code::Internal };
            return fail(ctx, code, &format!("{e:#}"), None);
        }
    };
    if live == 0 {
        return fail(
            ctx,
            Code::NotFound,
            &format!("team {team} has no running bots"),
            Some(&format!("Start them with `agents up {team}`, or run `agents wall` for every running agent.")),
        );
    }
    let code = crate::exec::run_pane_interactive(vec!["--session".into(), "ao".into()]);
    let _ = allternit_factory_pane::factory_backend::clear_wall_view(team);
    code
}

/// `orchestration transcript <to> [--tail N]`: the session's recorded
/// terminal transcript (it outlives the pane).
pub fn transcript(ctx: &Ctx, to: String, tail: Option<usize>) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let (session, entry) = match local_session(ctx, &rt, &to) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let Some(log) = entry.and_then(|e| e.log) else {
        return fail(ctx, Code::NotFound, &format!("{session} has no recorded transcript"), None);
    };
    let text = match std::fs::read(&log) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return fail(ctx, Code::NotFound, &format!("reading {log}: {e}"), None),
    };
    let text = match tail {
        Some(n) => {
            let lines: Vec<&str> = text.split_inclusive('\n').collect();
            lines[lines.len().saturating_sub(n)..].concat()
        }
        None => text,
    };
    if ctx.json {
        return ok_json(json!({ "session": session, "path": log, "text": text }));
    }
    print!("{text}");
    0
}

/// `orchestration drain <to> [--all]`: deliver the agent's queued messages
/// (oldest first) through the verified paste; each is settled only once its
/// delivery is verified, and the first that can't be delivered stays queued.
pub fn drain(ctx: &Ctx, to: String, all: bool, dry_run: bool) -> u8 {
    let rt = match runtime(ctx) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let (session, _) = match local_session(ctx, &rt, &to) {
        Ok(s) => s,
        Err(code) => return code,
    };
    drop(rt);
    let root = ctx.root_dir();
    if dry_run {
        let queued = match backend::backend().and_then(|b| b.mailbox(&root, &session)) {
            Ok(q) => q,
            Err(e) => return engine_err(ctx, &e),
        };
        let would: Vec<&str> = queued.iter().take(if all { usize::MAX } else { 1 }).map(|q| q.id.as_str()).collect();
        if ctx.json {
            return ok_json(json!({ "dryRun": true, "session": session, "queued": queued.len(), "wouldTry": would, "changed": false }));
        }
        println!("dry run: {} queued for {session}; would try {}", queued.len(), if would.is_empty() { "none".to_string() } else { would.join(", ") });
        println!("nothing was changed");
        return 0;
    }
    let drained = match backend::drain(&root, &session, all) {
        Ok(d) => d,
        Err(e) => return engine_err(ctx, &e),
    };
    // Keep the registry's queue depth current for `agents ps`.
    if let Ok(left) = backend::backend().and_then(|b| b.mailbox(&root, &session)) {
        let depth = left.len() as u32;
        let _ = Registry::open_default().update(|f| {
            if let Some(e) = f.sessions.get_mut(&session) {
                e.queued = depth;
            }
        });
    }
    if ctx.json {
        return ok_json(json!({ "session": session, "drained": drained }));
    }
    if drained.is_empty() {
        println!("no queued messages for {session}");
    }
    for d in &drained {
        if d.delivered {
            println!("delivered {} to {session}", d.message_id);
        } else {
            println!("left {} queued: the pane is busy, gone, or the paste could not be verified", d.message_id);
        }
    }
    0
}
