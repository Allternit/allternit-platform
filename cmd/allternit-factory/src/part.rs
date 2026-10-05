//! Part verbs implemented by the engine itself (not passed through to the
//! maintenance CLI or the pane CLI): `orchestration send` and `agents ps`.

use serde_json::json;

use allternit_factory_engine::registry::Registry;
use allternit_factory_engine::send::{self as send_mod, ApiLink, SendCtx, SendRequest};
use allternit_factory_engine::view;

use crate::exec::{fail, ok_json, Code, Ctx};

fn runtime(ctx: &Ctx) -> Result<tokio::runtime::Runtime, u8> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(ctx, Code::Internal, &format!("async runtime: {e}"), None))
}

/// Who is sending from the command line.
fn cli_sender() -> String {
    for var in ["AO_LEAD", "USER", "LOGNAME"] {
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
    if ctx.json {
        return ok_json(json!({ "agents": snap.agents, "engine": snap.engine }));
    }
    if let Some(err) = &snap.engine.error {
        eprintln!("pane engine: {err}");
    }
    for change in &snap.reconciled {
        eprintln!("reconciled {}: {}", change.session, change.kind);
    }
    if snap.agents.is_empty() {
        println!("no agents");
    }
    for a in &snap.agents {
        println!(
            "{:<24} {:<9} {:<10} {}",
            a.address,
            a.state,
            a.binding.harness.as_deref().unwrap_or("-"),
            a.pane.as_ref().map(|p| p.id.as_str()).unwrap_or("-")
        );
    }
    0
}
