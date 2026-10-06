//! `workflows` and `workspace` verbs that run in this process: templates
//! (built-ins included), `workflows run --team`, the `drive --team` pre-check,
//! proof add/show, the board, node pages, and `node list --mine`.
//!
//! Writes go through the workspace Gate (`open_workspace_gate`, the same
//! stores `plan new` uses). Reads are pure functions of the ledger and the
//! node folders; a read never creates `.allternit/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::agents::team::{self, TeamError};
use allternit_factory_engine::agents::team_apply::{self, ApiClient};
use allternit_factory_engine::agents::whoami;
use allternit_factory_engine::core::types::{AllternitEvent, LedgerQuery};
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::templates::{
    parse_param_args, plan_from_template_with_roles, Template, TemplateStore, TEMPLATE_DIR,
};
use allternit_factory_engine::workspace::{board, node_folder, node_page, proof};
use allternit_factory_engine::{Gate, Ledger};
use clap::Subcommand;
use serde_json::{json, Value};

use crate::exec::{fail, ok_json, Code, Ctx};

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().enable_all().build()?)
}

/// Every ledger event of the workspace; empty (and nothing created) when the
/// workspace has no ledger yet.
pub fn read_events(root: &Path) -> anyhow::Result<Vec<AllternitEvent>> {
    if !root.join(".allternit/ledger").is_dir() {
        return Ok(vec![]);
    }
    let ledger = Ledger::new(LedgerOptions {
        root_dir: Some(root.to_path_buf()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    });
    runtime()?.block_on(ledger.query(LedgerQuery::default()))
}

fn with_gate<T>(root: &Path, f: impl FnOnce(Arc<Gate>) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<T>>>>) -> anyhow::Result<T> {
    let rt = runtime()?;
    rt.block_on(async {
        let (_ledger, gate) = allternit_factory_engine::cli::rails::open_workspace_gate(root).await?;
        f(gate).await
    })
}

fn print(ctx: &Ctx, doc: Value) -> u8 {
    if ctx.json {
        ok_json(doc)
    } else {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        0
    }
}

fn team_fail(ctx: &Ctx, e: &TeamError) -> u8 {
    match e {
        TeamError::NotFound(t) => fail(ctx, Code::NotFound, &format!("team {t} not found (no .allternit/teams/{t}/team.yaml)"), None),
        _ => fail(ctx, Code::Usage, &e.to_string(), Some("Fix team.yaml (every problem is listed) and retry.")),
    }
}

// ─── Templates ─────────────────────────────────────────────────────────────────

/// `workflows template list|show|check`. The store falls back to the
/// built-in templates and never creates its directory.
pub fn template(ctx: &Ctx, cmd: &str, template: Option<&str>) -> u8 {
    let root = ctx.root_dir();
    let store = match TemplateStore::new(&root) {
        Ok(s) => s,
        Err(e) => return fail(ctx, Code::Internal, &format!("{e:#}"), None),
    };
    if cmd == "list" {
        let templates = match store.list() {
            Ok(t) => t,
            Err(e) => return fail(ctx, Code::Usage, &format!("{e:#}"), None),
        };
        if ctx.json {
            let list: Vec<Value> = templates
                .iter()
                .map(|t| json!({ "id": t.id, "name": t.name, "description": t.description, "builtin": t.builtin, "stepCount": t.steps.len() }))
                .collect();
            return ok_json(json!({ "templates": list }));
        }
        if templates.is_empty() {
            println!("no templates in {}", root.join(TEMPLATE_DIR).display());
        }
        for t in &templates {
            println!("{}\t{}\t{} steps{}", t.id, t.name, t.steps.len(), if t.builtin { "\t(built-in)" } else { "" });
        }
        return 0;
    }
    let id = template.unwrap_or_default();
    let t = match store.resolve(id) {
        Ok(t) => t,
        Err(e) => {
            let text = format!("{e:#}");
            let code = if text.contains("not found") { Code::NotFound } else { Code::Usage };
            return fail(ctx, code, &text, None);
        }
    };
    if cmd == "check" {
        if let Err(e) = t.validate() {
            return fail(ctx, Code::Usage, &format!("{e:#}"), None);
        }
    }
    if ctx.json {
        return ok_json(t.to_contract_json());
    }
    println!("{} — {} ({} steps){}", t.id, t.name, t.steps.len(), if t.builtin { " (built-in)" } else { "" });
    for p in &t.params {
        println!("  param {}", serde_json::to_string(p).unwrap_or_default());
    }
    for s in &t.steps {
        println!("  step {} — {} [{}]", s.id, s.title, s.executor.as_deref().unwrap_or("no executor"));
    }
    0
}

/// `role:` executors a template uses, in step order, deduped.
fn template_roles(t: &Template) -> Vec<String> {
    let mut seen = BTreeSet::new();
    t.steps
        .iter()
        .filter_map(|s| s.executor.as_deref()?.strip_prefix("role:").map(str::to_string))
        .filter(|r| seen.insert(r.clone()))
        .collect()
}

pub struct RunArgs {
    pub template: String,
    pub params: Vec<String>,
    pub text: Option<String>,
    pub intent: Option<String>,
    pub team: Option<String>,
    pub preset: Option<String>,
    pub dry_run: bool,
}

/// `--intent T` fills the template's `intent` param (when it declares one and
/// `--param intent=` wasn't given) and is the plan text.
fn effective_params(t: &Template, a: &RunArgs) -> (Vec<String>, Option<String>) {
    let mut params = a.params.clone();
    let text = a.text.clone().or(a.intent.clone());
    if let Some(intent) = &a.intent {
        let given = params.iter().any(|p| p.split_once('=').map(|(k, _)| k.trim() == "intent").unwrap_or(false));
        if !given && t.params.iter().any(|p| p.name == "intent") {
            params.push(format!("intent={intent}"));
        }
    }
    (params, text)
}

/// `workflows run`. Returns `None` when the caller should use the rails
/// passthrough (no `--team`, no role executors); the params/text to pass are
/// written back into `a`.
pub fn run(ctx: &Ctx, a: &mut RunArgs) -> Option<u8> {
    let root = ctx.root_dir();
    let store = match TemplateStore::new(&root) {
        Ok(s) => s,
        Err(e) => return Some(fail(ctx, Code::Internal, &format!("{e:#}"), None)),
    };
    let t = match store.resolve(&a.template) {
        Ok(t) => t,
        Err(e) => {
            let text = format!("{e:#}");
            let code = if text.contains("not found") { Code::NotFound } else { Code::Usage };
            return Some(fail(ctx, code, &text, None));
        }
    };
    let (params, text) = effective_params(&t, a);
    let roles_used = template_roles(&t);
    let Some(team_name) = a.team.clone() else {
        if !roles_used.is_empty() {
            return Some(fail(
                ctx,
                Code::Usage,
                &format!(
                    "template {} assigns steps by role ({}); pass --team <team> so each role maps to a bot",
                    t.id,
                    roles_used.join(", ")
                ),
                Some("List teams under .allternit/teams/; each role must be held by exactly one bot in team.yaml."),
            ));
        }
        a.params = params;
        a.text = text;
        return None;
    };
    let loaded = match team::load_team(&root, &team_name) {
        Ok(t) => t,
        Err(e) => return Some(team_fail(ctx, &e)),
    };
    let roles = match loaded.role_executors(a.preset.as_deref()) {
        Ok(r) => r,
        Err(e) => return Some(team_fail(ctx, &e)),
    };
    let parsed = match parse_param_args(&params) {
        Ok(p) => p,
        Err(e) => return Some(fail(ctx, Code::Usage, &format!("{e:#}"), None)),
    };
    // Validate (params, roles) before anything is written.
    if let Err(e) = t.expand_dag_with_roles("__root__", &parsed, Some(&roles)) {
        return Some(fail(ctx, Code::Usage, &format!("{e:#}"), None));
    }
    if a.dry_run {
        let nodes: Vec<Value> = t
            .steps
            .iter()
            .map(|s| {
                json!({
                    "stepId": s.id,
                    "title": s.title,
                    "executor": t.resolve_step_executor(s, Some(&roles)).ok().flatten(),
                    "blockedBy": s.blocked_by,
                })
            })
            .collect();
        return Some(print(ctx, json!({ "dryRun": true, "template": t.id, "team": loaded.name, "roles": roles, "nodes": nodes, "changed": false })));
    }
    let result = with_gate(&root, move |gate| {
        Box::pin(async move { plan_from_template_with_roles(&gate, &t, &parsed, text.as_deref(), None, None, Some(&roles)).await })
    });
    Some(match result {
        Ok(r) => {
            let nodes: Vec<Value> = r.nodes.iter().map(|(step, node)| json!({ "stepId": step, "nodeId": node })).collect();
            print(ctx, json!({ "dagId": r.dag_id, "campaignId": null, "rootNodeId": r.root_node_id, "nodes": nodes }))
        }
        Err(e) => {
            let code = if allternit_factory_engine::gate::GateError::from_anyhow(&e).is_some() { Code::Refused } else { Code::Internal };
            fail(ctx, code, &format!("{e:#}"), None)
        }
    })
}

/// `workflows drive … --team T`: check the team and the allternit-api
/// configuration before handing over to drive, so a team with vendor bots and
/// no API exits 3 (transport) instead of falling back to mail.
pub fn drive_precheck(ctx: &Ctx, args: &[String]) -> Option<u8> {
    let pos = args.iter().position(|a| a == "--team")?;
    // Passthrough args carry their own --root / --json (clap's globals stop
    // at a trailing argument list).
    let root = args
        .iter()
        .position(|a| a == "--root")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| args.iter().find_map(|a| a.strip_prefix("--root=").map(str::to_string)))
        .map(PathBuf::from)
        .or_else(|| ctx.root.clone());
    let local = Ctx::new(root, ctx.json || args.iter().any(|a| a == "--json"));
    let ctx = &local;
    let Some(team_name) = args.get(pos + 1) else {
        return Some(fail(ctx, Code::Usage, "--team needs a team name", None));
    };
    let preset = args.iter().position(|a| a == "--preset").and_then(|i| args.get(i + 1)).map(String::as_str);
    let dry = args.iter().any(|a| a == "--dry-run");
    let loaded = match team::load_team(&ctx.root_dir(), team_name) {
        Ok(t) => t,
        Err(e) => return Some(team_fail(ctx, &e)),
    };
    let vendors = match team_apply::vendor_bots(&loaded, preset) {
        Ok(v) => v,
        Err(e) => return Some(fail(ctx, Code::Usage, &format!("{e:#}"), None)),
    };
    match ApiClient::from_env() {
        Err(e) => Some(fail(ctx, Code::Usage, &e.fact, Some(team_apply::API_ENV_ACTION))),
        Ok(None) if !vendors.is_empty() && !dry => Some(fail(
            ctx,
            Code::Transport,
            &format!(
                "team {} has vendor bots ({}) but {}; drive would have to mail them instead, so it does not start",
                loaded.name,
                vendors.keys().cloned().collect::<Vec<_>>().join(", "),
                team_apply::API_NOT_SET_FACT
            ),
            Some(team_apply::API_ENV_ACTION),
        )),
        _ => None,
    }
}

// ─── Workspace ─────────────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ProofCmd {
    /// Copy a file into the node's proof/ as evidence for one Proof contract line.
    Add {
        node: String,
        /// The Proof contract line (text or its 1-based number).
        line: String,
        file: PathBuf,
        #[arg(long)]
        dag: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// The node's Proof contract with its evidence, and the proof/ files.
    Show {
        node: String,
        #[arg(long)]
        dag: Option<String>,
    },
}

/// The DAG holding `node`: `--dag` when given, else the one DAG whose ledger
/// created a node of that id.
fn dag_of(ctx: &Ctx, events: &[AllternitEvent], node: &str, dag: Option<String>) -> Result<String, u8> {
    if let Some(d) = dag {
        return Ok(d);
    }
    let dags: BTreeSet<String> = events
        .iter()
        .filter(|e| e.r#type == "DagNodeCreated" && e.payload.get("node_id").and_then(Value::as_str) == Some(node))
        .filter_map(|e| e.payload.get("dag_id").and_then(Value::as_str).map(str::to_string))
        .collect();
    match dags.len() {
        0 => Err(fail(ctx, Code::NotFound, &format!("no node {node} in this workspace"), Some("List nodes with `workspace node list`."))),
        1 => Ok(dags.into_iter().next().unwrap()),
        _ => Err(fail(
            ctx,
            Code::Usage,
            &format!("node {node} is in more than one DAG ({}); pass --dag", dags.into_iter().collect::<Vec<_>>().join(", ")),
            None,
        )),
    }
}

fn events_or_fail(ctx: &Ctx) -> Result<Vec<AllternitEvent>, u8> {
    read_events(&ctx.root_dir()).map_err(|e| fail(ctx, Code::Internal, &format!("reading the ledger: {e:#}"), None))
}

pub fn proof_cmd(ctx: &Ctx, cmd: ProofCmd) -> u8 {
    let events = match events_or_fail(ctx) {
        Ok(e) => e,
        Err(c) => return c,
    };
    let root = ctx.root_dir();
    match cmd {
        ProofCmd::Show { node, dag } => {
            let dag = match dag_of(ctx, &events, &node, dag) {
                Ok(d) => d,
                Err(c) => return c,
            };
            match node_page::build(&root, &events, &dag, &node) {
                Ok(p) => print(
                    ctx,
                    json!({
                        "dagId": dag,
                        "nodeId": node,
                        "proofContract": p.spec.map(|s| s.proof_contract).unwrap_or_default(),
                        "files": p.files,
                        "proof": p.card.proof,
                    }),
                ),
                Err(e) => fail(ctx, Code::NotFound, &format!("{e:#}"), None),
            }
        }
        ProofCmd::Add { node, line, file, dag, dry_run } => {
            let dag = match dag_of(ctx, &events, &node, dag) {
                Ok(d) => d,
                Err(c) => return c,
            };
            let bytes = match std::fs::read(&file) {
                Ok(b) => b,
                Err(e) => return fail(ctx, Code::NotFound, &format!("proof file {} not readable: {e}", file.display()), None),
            };
            if dry_run {
                // Same checks proof add makes, without the Gate or any write.
                if let Err(e) = node_page::build(&root, &events, &dag, &node) {
                    return fail(ctx, Code::NotFound, &format!("{e:#}"), None);
                }
                let spec = node_folder::read_spec(&root, &dag, &node).unwrap_or_default();
                let Some(matched) = proof::match_contract_line(&spec.proof_contract, &line) else {
                    return fail(
                        ctx,
                        Code::NotFound,
                        &format!("no Proof contract line {line:?} in {}/SPEC.md", node_folder::node_folder_rel_path(&dag, &node)),
                        Some("Add the line under `## Proof contract` first."),
                    );
                };
                let source = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
                let to = format!(
                    "{}/{}/{}",
                    node_folder::node_folder_rel_path(&dag, &node),
                    node_folder::PROOF_DIR,
                    proof::stored_proof_name(&bytes, &source)
                );
                return print(
                    ctx,
                    json!({ "dryRun": true, "changed": false, "line": matched.line, "wouldCopy": { "from": file, "to": to }, "sizeBytes": bytes.len() }),
                );
            }
            let (d, n) = (dag.clone(), node.clone());
            let res = with_gate(&root, move |gate| Box::pin(async move { proof::add(&gate, &d, &n, &line, &file).await }));
            match res {
                Ok(added) => print(ctx, serde_json::to_value(added).unwrap_or_default()),
                Err(e) => {
                    let text = format!("{e:#}");
                    let code = if allternit_factory_engine::gate::GateError::from_anyhow(&e).is_some() {
                        Code::Refused
                    } else if text.contains("not found") || text.contains("no Proof contract line") || text.contains("not readable") {
                        Code::NotFound
                    } else {
                        Code::Internal
                    };
                    fail(ctx, code, &text, None)
                }
            }
        }
    }
}

pub fn board_cmd(ctx: &Ctx, campaign: &str) -> u8 {
    let events = match events_or_fail(ctx) {
        Ok(e) => e,
        Err(c) => return c,
    };
    let b = match board::build(&ctx.root_dir(), &events, campaign) {
        Ok(b) => b,
        Err(e) => return fail(ctx, Code::NotFound, &format!("{e:#}"), Some("Pass a campaign id or a DAG id.")),
    };
    if ctx.json {
        return ok_json(serde_json::to_value(&b).unwrap_or_default());
    }
    println!("{} — {}", b.campaign.id, b.campaign.title);
    println!("proven {}/{} · needs you {}", b.summary.proven.k, b.summary.proven.n, b.summary.needs_you.len());
    for w in &b.waves {
        println!("wave {}:", w.depth);
        for c in &w.nodes {
            let status = serde_json::to_value(c.status).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            println!(
                "  {:<10} {} {} ({}/{} proven){}",
                status,
                c.node_id,
                c.title,
                c.proof.proven,
                c.proof.total,
                c.assignee.as_deref().map(|a| format!(" · {a}")).unwrap_or_default()
            );
        }
    }
    0
}

pub fn node_show(ctx: &Ctx, dag: &str, node: &str) -> u8 {
    let events = match events_or_fail(ctx) {
        Ok(e) => e,
        Err(c) => return c,
    };
    match node_page::build(&ctx.root_dir(), &events, dag, node) {
        Ok(p) => print(ctx, serde_json::to_value(p).unwrap_or_default()),
        Err(e) => fail(ctx, Code::NotFound, &format!("{e:#}"), None),
    }
}

/// `workspace node list --mine`: the open nodes of the bot this pane is.
pub fn node_list_mine(ctx: &Ctx) -> u8 {
    let w = match whoami::whoami_from_env(|k| std::env::var(k).ok()) {
        Ok(w) => w,
        Err(e) => {
            return fail(
                ctx,
                Code::NotFound,
                &format!("not inside a factory pane: {e}"),
                Some("Run this inside a pane started by `agents up`, or list without --mine."),
            )
        }
    };
    let events = match events_or_fail(ctx) {
        Ok(e) => e,
        Err(c) => return c,
    };
    let cards = board::cards_for_assignee(&ctx.root_dir(), &events, &w.address, true);
    if ctx.json {
        return ok_json(json!({ "nodes": cards }));
    }
    if cards.is_empty() {
        println!("no open nodes for {}", w.address);
    }
    for c in &cards {
        println!("{} {} {}", c.dag_id, c.node_id, c.title);
    }
    0
}
