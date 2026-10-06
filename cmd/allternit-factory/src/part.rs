//! Part verbs implemented by the engine itself (not passed through to the
//! maintenance CLI or the pane CLI): `orchestration send|capture|transcript|drain`
//! and `agents ps|down|recover`.

use serde_json::json;

use allternit_factory_engine::registry::Registry;
use allternit_factory_engine::send::{self as send_mod, ApiLink, SendCtx, SendRequest};
use allternit_factory_engine::view;
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
