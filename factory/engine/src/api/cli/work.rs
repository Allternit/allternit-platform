use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Subcommand;
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::ids::create_event_id;
use crate::core::io::{ensure_dir, read_json_file, write_json_atomic};
use crate::work::graph::ready_nodes;
use crate::work::projection::project_dag;
use crate::work::types::{DagNode, DagState};
use crate::{
    Actor, ActorType, AllternitEvent, DagMutation, Gate, Index, Ledger, LedgerQuery, WorkOps,
};

#[derive(Subcommand)]
pub enum WorkCmd {
    Info,
    Create {
        title: String,
        #[arg(long)]
        dag: Option<String>,
        #[arg(long)]
        parent: Option<String>,
        #[arg(short = 'p', long)]
        priority: Option<i64>,
        #[arg(short = 't', long)]
        kind: Option<String>,
        #[arg(short = 'd', long)]
        description: Option<String>,
        #[arg(short = 'l', long)]
        labels: Option<String>,
    },
    Update {
        id: String,
        #[arg(long)]
        dag: Option<String>,
        #[arg(long)]
        node: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(short = 'p', long)]
        priority: Option<i64>,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long = "spec-id")]
        spec_id: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        acceptance: Option<String>,
        #[arg(long)]
        design: Option<String>,
        #[arg(long)]
        claim: bool,
    },
    Close {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Reopen {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Show {
        id: String,
        #[arg(long)]
        dag: Option<String>,
        #[arg(long)]
        node: Option<String>,
    },
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long = "label-any")]
        label_any: Option<String>,
        #[arg(long)]
        priority: Option<i64>,
        #[arg(long)]
        all: bool,
    },
    Ready {
        #[arg(long)]
        nodes: bool,
    },
    Dep {
        #[command(subcommand)]
        command: DepCommand,
    },
    Label {
        #[command(subcommand)]
        command: LabelCommand,
    },
    Comment {
        id: String,
        text: String,
    },
    Comments {
        id: String,
    },
    State {
        id: String,
        dimension: String,
    },
    SetState {
        id: String,
        dimension_value: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    History {
        id: String,
    },
    Graph {
        id: String,
        #[arg(long, default_value = "json")]
        format: String,
    },
    Get {
        key: String,
    },
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    Export {
        file: PathBuf,
    },
    Import {
        file: PathBuf,
    },
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    Epic {
        title: String,
    },
    Mol {
        name: String,
        #[arg(long)]
        dag: Option<String>,
        nodes: Vec<String>,
    },
    Swarm {
        name: String,
        #[arg(long)]
        dag: Option<String>,
        nodes: Vec<String>,
    },
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Sync {
        #[arg(long)]
        peer: Option<String>,
    },
    Federation {
        #[command(subcommand)]
        command: FederationCommand,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
pub enum DepCommand {
    Add {
        child: String,
        parent: String,
        #[arg(long)]
        dag: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum LabelCommand {
    Add { id: String, label: String },
    Remove { id: String, label: String },
    List { id: String },
}

#[derive(Subcommand)]
pub enum HookCommand {
    Add {
        name: String,
        command: String,
    },
    Remove {
        name: String,
    },
    List,
    Run {
        name: String,
        #[arg(long)]
        exec: bool,
    },
}

#[derive(Subcommand)]
pub enum TemplateCommand {
    Save {
        template_id: String,
        #[arg(long)]
        dag: String,
    },
    Show {
        template_id: String,
    },
    Instantiate {
        template_id: String,
        title: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    Start,
    Stop,
    Status,
}

#[derive(Subcommand)]
pub enum FederationCommand {
    AddPeer { name: String, url: String },
    RemovePeer { name: String },
    ListPeers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HookEntry {
    name: String,
    command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HooksFile {
    hooks: Vec<HookEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemplateFile {
    template_id: String,
    title: String,
    dag: DagState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FederationFile {
    peers: Vec<FederationPeer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FederationPeer {
    name: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonState {
    status: String,
    updated_at: String,
}

pub struct WorkContext {
    pub root: PathBuf,
    pub ledger: Arc<Ledger>,
    pub gate: Arc<Gate>,
    pub work: Arc<WorkOps>,
    pub index: Arc<Index>,
}

pub async fn run_work_command(ctx: &WorkContext, cmd: WorkCmd) -> Result<()> {
    match cmd {
        WorkCmd::Info => info(ctx).await?,
        WorkCmd::Create {
            title,
            dag,
            parent,
            priority,
            kind,
            description,
            labels,
        } => {
            create_issue(
                ctx,
                &title,
                dag,
                parent,
                priority,
                kind,
                description,
                labels,
            )
            .await?;
        }
        WorkCmd::Update {
            id,
            dag,
            node,
            status,
            priority,
            assignee,
            spec_id,
            title,
            description,
            notes,
            acceptance,
            design,
            claim,
        } => {
            update_issue(
                ctx,
                &id,
                dag,
                node,
                status,
                priority,
                assignee,
                spec_id,
                title,
                description,
                notes,
                acceptance,
                design,
                claim,
            )
            .await?;
        }
        WorkCmd::Close { id, reason } => close_issue(ctx, &id, reason).await?,
        WorkCmd::Reopen { id, reason } => reopen_issue(ctx, &id, reason).await?,
        WorkCmd::Show { id, dag, node } => show_issue(ctx, &id, dag, node).await?,
        WorkCmd::List {
            status,
            assignee,
            label,
            label_any,
            priority,
            all,
        } => {
            list_issues(ctx, status, assignee, label, label_any, priority, all).await?;
        }
        WorkCmd::Ready { nodes } => ready_issues(ctx, nodes).await?,
        WorkCmd::Dep { command } => match command {
            DepCommand::Add { child, parent, dag } => {
                add_dependency(ctx, &child, &parent, dag).await?
            }
        },
        WorkCmd::Label { command } => match command {
            LabelCommand::Add { id, label } => label_add(ctx, &id, &label).await?,
            LabelCommand::Remove { id, label } => label_remove(ctx, &id, &label).await?,
            LabelCommand::List { id } => label_list(ctx, &id).await?,
        },
        WorkCmd::Comment { id, text } => comment_add(ctx, &id, &text).await?,
        WorkCmd::Comments { id } => comment_list(ctx, &id).await?,
        WorkCmd::State { id, dimension } => state_get(ctx, &id, &dimension).await?,
        WorkCmd::SetState {
            id,
            dimension_value,
            reason,
        } => state_set(ctx, &id, &dimension_value, reason).await?,
        WorkCmd::Search { query, limit } => search_events(ctx, &query, limit).await?,
        WorkCmd::History { id } => history(ctx, &id).await?,
        WorkCmd::Graph { id, format } => graph(ctx, &id, &format).await?,
        WorkCmd::Get { key } => kv_get(ctx, &key).await?,
        WorkCmd::Set { key, value } => kv_set(ctx, &key, &value).await?,
        WorkCmd::Unset { key } => kv_unset(ctx, &key).await?,
        WorkCmd::Export { file } => export_events(ctx, &file).await?,
        WorkCmd::Import { file } => import_events(ctx, &file).await?,
        WorkCmd::Hook { command } => hooks_cmd(ctx, command).await?,
        WorkCmd::Template { command } => templates_cmd(ctx, command).await?,
        WorkCmd::Epic { title } => epic_create(ctx, &title).await?,
        WorkCmd::Mol { name, dag, nodes } => group_label(ctx, "mol", &name, dag, nodes).await?,
        WorkCmd::Swarm { name, dag, nodes } => group_label(ctx, "swarm", &name, dag, nodes).await?,
        WorkCmd::Daemon { command } => daemon_cmd(ctx, command).await?,
        WorkCmd::Sync { peer } => sync_cmd(ctx, peer).await?,
        WorkCmd::Federation { command } => federation_cmd(ctx, command).await?,
        WorkCmd::External(args) => external_cmd(ctx, &args).await?,
    }

    Ok(())
}

async fn info(ctx: &WorkContext) -> Result<()> {
    let db_path = ctx.root.join(".allternit/ledger");
    println!("database_path: {}", db_path.display());
    println!("issue_prefix: {}", "dag");
    Ok(())
}

async fn create_issue(
    ctx: &WorkContext,
    title: &str,
    dag: Option<String>,
    parent: Option<String>,
    priority: Option<i64>,
    kind: Option<String>,
    description: Option<String>,
    labels: Option<String>,
) -> Result<()> {
    if let Some(dag_id) = dag {
        let node_id = format!("n_{}", random::<u32>() % 1_000_000);
        let mut patch = serde_json::Map::new();
        if let Some(priority) = priority {
            patch.insert("priority".to_string(), json!(priority));
        }
        if let Some(desc) = description.clone() {
            patch.insert("description".to_string(), json!(desc));
        }
        if let Some(labels) = labels.as_ref() {
            let list = labels
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            patch.insert("labels".to_string(), json!(list));
        }
        let node_kind = kind.unwrap_or_else(|| "task".to_string());
        let mutations = vec![DagMutation::CreateNode {
            node_id: node_id.clone(),
            node_kind,
            title: title.to_string(),
            parent_node_id: parent,
            execution_mode: "shared".to_string(),
            description: None,
            executor: None,
        }];
        let _ = ctx
            .gate
            .mutate_with_decision(&dag_id, "allternit work create", None, mutations)
            .await?;
        if !patch.is_empty() {
            let _ = ctx
                .gate
                .mutate_with_decision(
                    &dag_id,
                    "allternit work update",
                    None,
                    vec![DagMutation::UpdateNode {
                        node_id,
                        patch: json!(patch),
                    }],
                )
                .await?;
        }
        println!("{dag_id}");
        return Ok(());
    }

    let (prompt_id, dag_id, node_id) = ctx.gate.plan_new(title, None).await?;
    if priority.is_some() || description.is_some() || labels.is_some() || kind.is_some() {
        let mut patch = serde_json::Map::new();
        if let Some(priority) = priority {
            patch.insert("priority".to_string(), json!(priority));
        }
        if let Some(desc) = description {
            patch.insert("description".to_string(), json!(desc));
        }
        if let Some(labels) = labels {
            let list = labels
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            patch.insert("labels".to_string(), json!(list));
        }
        if let Some(kind) = kind {
            patch.insert("node_kind".to_string(), json!(kind));
        }
        if !patch.is_empty() {
            let _ = ctx
                .gate
                .mutate_with_decision(
                    &dag_id,
                    "allternit work update",
                    None,
                    vec![DagMutation::UpdateNode {
                        node_id: node_id.clone(),
                        patch: json!(patch),
                    }],
                )
                .await?;
        }
    }
    println!("prompt_id: {prompt_id}\ndag_id: {dag_id}\nnode_id: {node_id}");
    Ok(())
}

async fn update_issue(
    ctx: &WorkContext,
    id: &str,
    dag: Option<String>,
    node: Option<String>,
    status: Option<String>,
    priority: Option<i64>,
    assignee: Option<String>,
    spec_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    notes: Option<String>,
    acceptance: Option<String>,
    design: Option<String>,
    claim: bool,
) -> Result<()> {
    let dag_id = dag.unwrap_or_else(|| id.to_string());
    let node_id = resolve_node_id(ctx, &dag_id, node.as_deref()).await?;

    let mut patch = serde_json::Map::new();
    if let Some(priority) = priority {
        patch.insert("priority".to_string(), json!(priority));
    }
    if let Some(assignee) = assignee {
        patch.insert("assignee".to_string(), json!(assignee));
    }
    if let Some(spec_id) = spec_id {
        patch.insert("spec_id".to_string(), json!(spec_id));
    }
    if let Some(title) = title {
        patch.insert("title".to_string(), json!(title));
    }
    if let Some(description) = description {
        patch.insert("description".to_string(), json!(description));
    }
    if let Some(notes) = notes {
        patch.insert("notes".to_string(), json!(notes));
    }
    if let Some(acceptance) = acceptance {
        patch.insert("acceptance".to_string(), json!(acceptance));
    }
    if let Some(design) = design {
        patch.insert("design".to_string(), json!(design));
    }
    let mut status_override = status.clone();
    if claim {
        patch.insert("assignee".to_string(), json!("cli"));
        if status_override.is_none() {
            status_override = Some("IN_PROGRESS".to_string());
        }
    }

    let mut mutations = Vec::new();
    if let Some(status) = status_override {
        let from = current_node_status(ctx, &dag_id, &node_id).await?;
        crate::kernel::lifecycle::check_legacy_change(&from, &status)?;
        mutations.push(DagMutation::ChangeStatus {
            node_id: node_id.clone(),
            from,
            to: status,
            reason: None,
        });
    }
    if !patch.is_empty() {
        mutations.push(DagMutation::UpdateNode {
            node_id,
            patch: json!(patch),
        });
    }
    if !mutations.is_empty() {
        ctx.gate
            .mutate_with_decision(&dag_id, "allternit work update", None, mutations)
            .await?;
    }
    Ok(())
}

async fn close_issue(ctx: &WorkContext, id: &str, reason: Option<String>) -> Result<()> {
    let dag_id = id.to_string();
    let node_id = resolve_node_id(ctx, &dag_id, None).await?;
    let from = current_node_status(ctx, &dag_id, &node_id).await?;
    crate::kernel::lifecycle::check_legacy_change(&from, "DONE")?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work close",
            reason.clone(),
            vec![DagMutation::ChangeStatus {
                node_id,
                from,
                to: "DONE".to_string(),
                reason,
            }],
        )
        .await?;
    Ok(())
}

async fn reopen_issue(ctx: &WorkContext, id: &str, reason: Option<String>) -> Result<()> {
    let dag_id = id.to_string();
    let node_id = resolve_node_id(ctx, &dag_id, None).await?;
    let from = current_node_status(ctx, &dag_id, &node_id).await?;
    crate::kernel::lifecycle::check_legacy_change(&from, "NEW")?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work reopen",
            reason.clone(),
            vec![DagMutation::ChangeStatus {
                node_id,
                from,
                to: "NEW".to_string(),
                reason,
            }],
        )
        .await?;
    Ok(())
}

async fn show_issue(
    ctx: &WorkContext,
    id: &str,
    dag: Option<String>,
    node: Option<String>,
) -> Result<()> {
    let dag_id = dag.unwrap_or_else(|| id.to_string());
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag(&events, &dag_id);
    let dag_state = project_dag(&dag_events, &dag_id);
    if let Some(node_id) = node {
        if let Some(node) = dag_state.nodes.get(&node_id) {
            println!("{}", serde_json::to_string_pretty(node)?);
        } else {
            return Err(anyhow!("node not found"));
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&dag_state)?);
    }
    Ok(())
}

async fn list_issues(
    ctx: &WorkContext,
    status: Option<String>,
    assignee: Option<String>,
    label: Option<String>,
    label_any: Option<String>,
    priority: Option<i64>,
    all: bool,
) -> Result<()> {
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_ids = collect_dag_ids(&events);
    let mut rows = Vec::new();
    for dag_id in dag_ids {
        let dag_events = events_for_dag(&events, &dag_id);
        let dag = project_dag(&dag_events, &dag_id);
        let nodes: Vec<&DagNode> = if all {
            dag.nodes.values().collect()
        } else {
            root_nodes(&dag).collect()
        };
        for node in nodes {
            if let Some(status_filter) = status.as_ref() {
                if node.status != *status_filter {
                    continue;
                }
            }
            if let Some(assignee_filter) = assignee.as_ref() {
                if node.assignee.as_deref() != Some(assignee_filter) {
                    continue;
                }
            }
            if let Some(priority_filter) = priority {
                if node.priority.unwrap_or_default() != priority_filter {
                    continue;
                }
            }
            if let Some(label_filter) = label.as_ref() {
                if !node.labels.contains(label_filter) {
                    continue;
                }
            }
            if let Some(label_any) = label_any.as_ref() {
                let any_labels: HashSet<&str> = label_any
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !node.labels.iter().any(|l| any_labels.contains(l.as_str())) {
                    continue;
                }
            }
            rows.push((dag_id.clone(), node.node_id.clone(), node.title.clone()));
        }
    }

    for (dag_id, node_id, title) in rows {
        println!("{dag_id} {node_id} {title}");
    }
    Ok(())
}

async fn ready_issues(ctx: &WorkContext, nodes: bool) -> Result<()> {
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_ids = collect_dag_ids(&events);
    for dag_id in dag_ids {
        let dag_events = events_for_dag(&events, &dag_id);
        let dag = project_dag(&dag_events, &dag_id);
        if nodes {
            for node_id in ready_nodes(&dag) {
                if let Some(node) = dag.nodes.get(&node_id) {
                    println!("{dag_id} {node_id} {}", node.title);
                }
            }
        } else {
            for node in root_nodes(&dag) {
                if node.status == "READY" || node.status == "NEW" {
                    println!("{dag_id} {}", node.title);
                }
            }
        }
    }
    Ok(())
}

async fn add_dependency(
    ctx: &WorkContext,
    child: &str,
    parent: &str,
    dag: Option<String>,
) -> Result<()> {
    let dag_id = dag.unwrap_or_else(|| child.to_string());
    let child_node = resolve_node_id(ctx, &dag_id, Some(child)).await?;
    let parent_node = resolve_node_id(ctx, &dag_id, Some(parent)).await?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work dep",
            None,
            vec![DagMutation::AddBlockedBy {
                from_node_id: parent_node,
                to_node_id: child_node,
            }],
        )
        .await?;
    Ok(())
}

async fn label_add(ctx: &WorkContext, id: &str, label: &str) -> Result<()> {
    let dag_id = id.to_string();
    let node_id = resolve_node_id(ctx, &dag_id, None).await?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work label add",
            None,
            vec![DagMutation::AddLabel {
                node_id,
                label: label.to_string(),
            }],
        )
        .await?;
    Ok(())
}

async fn label_remove(ctx: &WorkContext, id: &str, label: &str) -> Result<()> {
    let dag_id = id.to_string();
    let node_id = resolve_node_id(ctx, &dag_id, None).await?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work label remove",
            None,
            vec![DagMutation::RemoveLabel {
                node_id,
                label: label.to_string(),
            }],
        )
        .await?;
    Ok(())
}

async fn label_list(ctx: &WorkContext, id: &str) -> Result<()> {
    let dag_id = id.to_string();
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag(&events, &dag_id);
    let dag = project_dag(&dag_events, &dag_id);
    let node_id = resolve_root_node_id(&dag).ok_or_else(|| anyhow!("root node not found"))?;
    let node = dag
        .nodes
        .get(&node_id)
        .ok_or_else(|| anyhow!("root node not found"))?;
    println!("{}", node.labels.join(","));
    Ok(())
}

async fn comment_add(ctx: &WorkContext, id: &str, text: &str) -> Result<()> {
    let dag_id = id.to_string();
    let node_id = resolve_node_id(ctx, &dag_id, None).await?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work comment",
            None,
            vec![DagMutation::AddComment {
                node_id,
                body_ref: text.to_string(),
                author: "cli".to_string(),
            }],
        )
        .await?;
    Ok(())
}

async fn comment_list(ctx: &WorkContext, id: &str) -> Result<()> {
    let dag_id = id.to_string();
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    for evt in events.iter().filter(|e| e.r#type == "CommentAdded") {
        if evt.payload.get("dag_id").and_then(|v| v.as_str()) != Some(&dag_id) {
            continue;
        }
        println!("{}", serde_json::to_string_pretty(evt)?);
    }
    Ok(())
}

async fn state_get(ctx: &WorkContext, id: &str, dimension: &str) -> Result<()> {
    let dag_id = id.to_string();
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag(&events, &dag_id);
    let dag = project_dag(&dag_events, &dag_id);
    let node_id = resolve_root_node_id(&dag).ok_or_else(|| anyhow!("root node not found"))?;
    let node = dag
        .nodes
        .get(&node_id)
        .ok_or_else(|| anyhow!("root node not found"))?;
    if let Some(value) = node.state.get(dimension) {
        println!("{value}");
    }
    Ok(())
}

async fn state_set(
    ctx: &WorkContext,
    id: &str,
    dimension_value: &str,
    reason: Option<String>,
) -> Result<()> {
    let dag_id = id.to_string();
    let node_id = resolve_node_id(ctx, &dag_id, None).await?;
    let mut parts = dimension_value.splitn(2, '=');
    let dimension = parts.next().unwrap_or("");
    let value = parts.next().unwrap_or("");
    if dimension.is_empty() {
        return Err(anyhow!("dimension required"));
    }
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "allternit work set-state",
            reason.clone(),
            vec![DagMutation::SetState {
                node_id,
                dimension: dimension.to_string(),
                value: value.to_string(),
                reason,
            }],
        )
        .await?;
    Ok(())
}

async fn search_events(ctx: &WorkContext, query: &str, limit: i64) -> Result<()> {
    let ids = ctx.index.search_events(query, limit).await?;
    for id in ids {
        println!("{id}");
    }
    Ok(())
}

async fn history(ctx: &WorkContext, id: &str) -> Result<()> {
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    for evt in events
        .iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(id))
    {
        println!("{}", serde_json::to_string_pretty(evt)?);
    }
    Ok(())
}

async fn graph(ctx: &WorkContext, id: &str, format: &str) -> Result<()> {
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag(&events, id);
    let dag = project_dag(&dag_events, id);
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&dag)?);
    } else {
        for node in dag.nodes.values() {
            println!("{} [{}]", node.node_id, node.title);
        }
    }
    Ok(())
}

async fn kv_get(ctx: &WorkContext, key: &str) -> Result<()> {
    let snapshot = ctx.work.kv_snapshot().await?;
    if let Some(value) = snapshot.get(key) {
        println!("{value}");
    }
    Ok(())
}

async fn kv_set(ctx: &WorkContext, key: &str, value: &str) -> Result<()> {
    ctx.work.kv_set(key, value, None).await?;
    Ok(())
}

async fn kv_unset(ctx: &WorkContext, key: &str) -> Result<()> {
    ctx.work.kv_unset(key, None).await?;
    Ok(())
}

async fn export_events(ctx: &WorkContext, file: &Path) -> Result<()> {
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    if let Some(parent) = file.parent() {
        ensure_dir(parent)?;
    }
    let mut out = String::new();
    for evt in events {
        out.push_str(&serde_json::to_string(&evt)?);
        out.push('\n');
    }
    std::fs::write(file, out)?;
    Ok(())
}

async fn import_events(ctx: &WorkContext, file: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(file)?;
    let existing = ctx.ledger.query(LedgerQuery::default()).await?;
    let mut seen = HashSet::new();
    for evt in existing {
        seen.insert(evt.event_id);
    }
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let evt: AllternitEvent = serde_json::from_str(line)?;
        if seen.contains(&evt.event_id) {
            continue;
        }
        ctx.ledger.append(evt).await?;
    }
    Ok(())
}

async fn hooks_cmd(ctx: &WorkContext, command: HookCommand) -> Result<()> {
    let hooks_path = ctx.root.join(".allternit/hooks/hooks.json");
    let mut hooks: HooksFile = read_json_file(&hooks_path, HooksFile::default())?;
    match command {
        HookCommand::Add { name, command } => {
            hooks.hooks.retain(|h| h.name != name);
            hooks.hooks.push(HookEntry { name, command });
            write_json_atomic(&hooks_path, &hooks)?;
        }
        HookCommand::Remove { name } => {
            hooks.hooks.retain(|h| h.name != name);
            write_json_atomic(&hooks_path, &hooks)?;
        }
        HookCommand::List => {
            println!("{}", serde_json::to_string_pretty(&hooks)?);
        }
        HookCommand::Run { name, exec } => {
            let hook = hooks
                .hooks
                .iter()
                .find(|h| h.name == name)
                .ok_or_else(|| anyhow!("hook not found"))?;
            if exec {
                let status = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&hook.command)
                    .status()?;
                println!("exit: {}", status.code().unwrap_or(-1));
            }
            let evt = compat_event(
                "allternit work",
                "hook run",
                json!({"hook": hook.name, "exec": exec}),
            );
            ctx.ledger.append(evt).await?;
        }
    }
    Ok(())
}

async fn templates_cmd(ctx: &WorkContext, command: TemplateCommand) -> Result<()> {
    let templates_dir = ctx.root.join(".allternit/templates");
    ensure_dir(&templates_dir)?;
    match command {
        TemplateCommand::Save { template_id, dag } => {
            let events = ctx.ledger.query(LedgerQuery::default()).await?;
            let dag_events = events_for_dag(&events, &dag);
            let dag_state = project_dag(&dag_events, &dag);
            let title = root_title(&dag_state).unwrap_or_else(|| "Template".to_string());
            let template = TemplateFile {
                template_id: template_id.clone(),
                title,
                dag: dag_state,
            };
            let path = templates_dir.join(format!("{template_id}.json"));
            write_json_atomic(&path, &template)?;
        }
        TemplateCommand::Show { template_id } => {
            let path = templates_dir.join(format!("{template_id}.json"));
            let template: TemplateFile = read_json_file(
                &path,
                TemplateFile {
                    template_id,
                    title: "".to_string(),
                    dag: DagState {
                        dag_id: "".to_string(),
                        nodes: HashMap::new(),
                        edges: Vec::new(),
                        relations: Vec::new(),
                    },
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&template)?);
        }
        TemplateCommand::Instantiate { template_id, title } => {
            let path = templates_dir.join(format!("{template_id}.json"));
            let template: TemplateFile = read_json_file(
                &path,
                TemplateFile {
                    template_id: template_id.clone(),
                    title: "".to_string(),
                    dag: DagState {
                        dag_id: "".to_string(),
                        nodes: HashMap::new(),
                        edges: Vec::new(),
                        relations: Vec::new(),
                    },
                },
            )?;
            let base_title = title.unwrap_or_else(|| template.title.clone());
            let (_prompt_id, dag_id, root_node) = ctx.gate.plan_new(&base_title, None).await?;
            let mut mutations = Vec::new();
            let mut node_map = HashMap::new();
            node_map.insert(
                resolve_root_node_id(&template.dag).unwrap_or_default(),
                root_node.clone(),
            );
            let root_template_id = resolve_root_node_id(&template.dag).unwrap_or_default();
            let mut template_nodes: Vec<_> = template
                .dag
                .nodes
                .values()
                .filter(|n| n.node_id != root_template_id)
                .collect();
            template_nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
            // Map every node id first so parents and `{{ <node>.output }}`
            // placeholders can reference nodes declared later.
            for (idx, node) in template_nodes.iter().enumerate() {
                node_map.insert(node.node_id.clone(), format!("n_{:04}", idx + 2));
            }
            for node in &template_nodes {
                let new_id = node_map[&node.node_id].clone();
                mutations.push(DagMutation::CreateNode {
                    node_id: new_id.clone(),
                    node_kind: node.node_kind.clone(),
                    title: node.title.clone(),
                    parent_node_id: node
                        .parent_node_id
                        .as_ref()
                        .and_then(|p| node_map.get(p))
                        .cloned(),
                    execution_mode: node.execution_mode.clone(),
                    description: node
                        .description
                        .as_deref()
                        .map(|d| crate::work::placeholders::rewrite_node_ids(d, &node_map)),
                    executor: node.executor.clone(),
                });
                for gate in &node.wait_gates {
                    mutations.push(DagMutation::AddWaitGate {
                        node_id: new_id.clone(),
                        gate_id: None,
                        kind: gate.kind.clone(),
                        description: Some(gate.description.clone()),
                        params: gate.params.clone(),
                    });
                }
            }
            for edge in template.dag.edges.iter() {
                if let (Some(from), Some(to)) = (
                    node_map.get(&edge.from_node_id),
                    node_map.get(&edge.to_node_id),
                ) {
                    mutations.push(DagMutation::AddBlockedBy {
                        from_node_id: from.clone(),
                        to_node_id: to.clone(),
                    });
                }
            }
            for rel in template.dag.relations.iter() {
                if let (Some(a), Some(b)) = (node_map.get(&rel.a), node_map.get(&rel.b)) {
                    mutations.push(DagMutation::AddRelation {
                        a: a.clone(),
                        b: b.clone(),
                        note: rel.note.clone(),
                        context_share: Some(rel.context_share),
                    });
                }
            }
            if !mutations.is_empty() {
                let _ = ctx
                    .gate
                    .mutate_with_decision(&dag_id, "template instantiate", None, mutations)
                    .await?;
            }
            println!("{dag_id}");
        }
    }
    Ok(())
}

async fn epic_create(ctx: &WorkContext, title: &str) -> Result<()> {
    let (_prompt_id, dag_id, node_id) = ctx.gate.plan_new(title, None).await?;
    let _ = ctx
        .gate
        .mutate_with_decision(
            &dag_id,
            "epic",
            None,
            vec![DagMutation::UpdateNode {
                node_id,
                patch: json!({ "node_kind": "epic" }),
            }],
        )
        .await?;
    println!("{dag_id}");
    Ok(())
}

async fn group_label(
    ctx: &WorkContext,
    prefix: &str,
    name: &str,
    dag: Option<String>,
    nodes: Vec<String>,
) -> Result<()> {
    let dag_id = dag.unwrap_or_else(|| name.to_string());
    let label = format!("{prefix}:{name}");
    let target_nodes = if nodes.is_empty() {
        vec![resolve_node_id(ctx, &dag_id, None).await?]
    } else {
        nodes
    };
    let mut mutations = Vec::new();
    for node_id in target_nodes {
        mutations.push(DagMutation::AddLabel {
            node_id,
            label: label.clone(),
        });
    }
    let _ = ctx
        .gate
        .mutate_with_decision(&dag_id, "group label", None, mutations)
        .await?;
    Ok(())
}

async fn daemon_cmd(ctx: &WorkContext, command: DaemonCommand) -> Result<()> {
    let path = ctx.root.join(".allternit/daemon/state.json");
    match command {
        DaemonCommand::Start => {
            let state = DaemonState {
                status: "running".to_string(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            write_json_atomic(&path, &state)?;
        }
        DaemonCommand::Stop => {
            let state = DaemonState {
                status: "stopped".to_string(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            write_json_atomic(&path, &state)?;
        }
        DaemonCommand::Status => {
            let state: DaemonState = read_json_file(
                &path,
                DaemonState {
                    status: "stopped".to_string(),
                    updated_at: "".to_string(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
    }
    Ok(())
}

async fn sync_cmd(ctx: &WorkContext, peer: Option<String>) -> Result<()> {
    let evt = compat_event("allternit work", "sync", json!({"peer": peer}));
    ctx.ledger.append(evt).await?;
    Ok(())
}

async fn federation_cmd(ctx: &WorkContext, command: FederationCommand) -> Result<()> {
    let path = ctx.root.join(".allternit/federation/peers.json");
    let mut data: FederationFile = read_json_file(&path, FederationFile::default())?;
    match command {
        FederationCommand::AddPeer { name, url } => {
            data.peers.retain(|p| p.name != name);
            data.peers.push(FederationPeer { name, url });
            write_json_atomic(&path, &data)?;
        }
        FederationCommand::RemovePeer { name } => {
            data.peers.retain(|p| p.name != name);
            write_json_atomic(&path, &data)?;
        }
        FederationCommand::ListPeers => {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
    }
    Ok(())
}

async fn external_cmd(ctx: &WorkContext, args: &[String]) -> Result<()> {
    let evt = compat_event("allternit work", "external", json!({"args": args}));
    ctx.ledger.append(evt).await?;
    Ok(())
}

fn compat_event(tool: &str, command: &str, payload: serde_json::Value) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: chrono::Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: tool.to_string(),
        },
        scope: None,
        r#type: "CompatCommandExecuted".to_string(),
        payload: json!({"tool": tool, "command": command, "payload": payload}),
        provenance: None,
    }
}

fn collect_dag_ids(events: &[AllternitEvent]) -> Vec<String> {
    let mut out = HashSet::new();
    for evt in events {
        if let Some(dag_id) = evt.payload.get("dag_id").and_then(|v| v.as_str()) {
            out.insert(dag_id.to_string());
        }
    }
    let mut list: Vec<String> = out.into_iter().collect();
    list.sort();
    list
}

fn events_for_dag(events: &[AllternitEvent], dag_id: &str) -> Vec<AllternitEvent> {
    events
        .iter()
        .filter(|evt| evt.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .cloned()
        .collect()
}

fn root_nodes(dag: &DagState) -> impl Iterator<Item = &DagNode> {
    dag.nodes.values().filter(|n| n.parent_node_id.is_none())
}

fn resolve_root_node_id(dag: &DagState) -> Option<String> {
    dag.nodes
        .values()
        .filter(|n| n.parent_node_id.is_none())
        .min_by_key(|n| n.created_at.clone())
        .map(|n| n.node_id.clone())
}

async fn current_node_status(ctx: &WorkContext, dag_id: &str, node_id: &str) -> Result<String> {
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag = project_dag(&events_for_dag(&events, dag_id), dag_id);
    dag.nodes
        .get(node_id)
        .map(|node| node.status.clone())
        .ok_or_else(|| anyhow!("node {node_id} not found in dag {dag_id}"))
}

async fn resolve_node_id(ctx: &WorkContext, dag_id: &str, node_id: Option<&str>) -> Result<String> {
    if let Some(node_id) = node_id {
        return Ok(node_id.to_string());
    }
    let events = ctx.ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag(&events, dag_id);
    let dag = project_dag(&dag_events, dag_id);
    resolve_root_node_id(&dag).ok_or_else(|| anyhow!("root node not found"))
}

fn root_title(dag: &DagState) -> Option<String> {
    resolve_root_node_id(dag).and_then(|id| dag.nodes.get(&id).map(|n| n.title.clone()))
}
