//! Campaign board (API.md Workspace: `GET /api/factory/campaigns/:id/board`).
//!
//! A pure function of (workspace root, ledger events): cards come from the
//! projected DAG, WIH, wait-gates and judge verdicts; proof counts come only
//! from receipts and verdicts (see `workspace::proof`), never from markdown
//! checkboxes. SPEC.md is read only for the number of Proof contract lines.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::campaign::project_campaigns;
use crate::core::types::AllternitEvent;
use crate::judge::status as judge_status;
use crate::wait_gates::WaitGateKind;
use crate::work::projection::project_dag;
use crate::work::types::{DagNode, DagState};
use crate::workspace::{node_folder, proof};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    New,
    Ready,
    Working,
    Checking,
    NeedsYou,
    Done,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofTally {
    pub proven: u32,
    pub total: u32,
}

/// Set when the node waits on a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardGate {
    /// `manual` (an unresolved manual wait-gate) or `judge` (a verdict
    /// handed the node to a person).
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeCard {
    pub dag_id: String,
    pub node_id: String,
    pub title: String,
    pub status: CardStatus,
    /// Agent address.
    pub assignee: Option<String>,
    /// `hosted` | `terminal` | `vendor`; null unless known.
    pub binding_type: Option<String>,
    pub proof: ProofTally,
    pub blocked_by: Vec<String>,
    pub needs_you: bool,
    /// Longest blocked_by chain above this node.
    pub depth: u32,
    pub gate: Option<CardGate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardCampaign {
    pub id: String,
    pub title: String,
    pub intent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenKN {
    pub k: u32,
    pub n: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary {
    pub now: Vec<NodeCard>,
    pub next: Vec<NodeCard>,
    pub proven: ProvenKN,
    pub needs_you: Vec<NodeCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wave {
    pub depth: u32,
    pub nodes: Vec<NodeCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub campaign: BoardCampaign,
    pub summary: BoardSummary,
    pub waves: Vec<Wave>,
}

/// Events whose payload names `dag_id`.
pub fn dag_events(events: &[AllternitEvent], dag_id: &str) -> Vec<AllternitEvent> {
    events
        .iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .cloned()
        .collect()
}

fn blockers(dag: &DagState, node_id: &str) -> Vec<String> {
    let mut v: Vec<String> = dag
        .edges
        .iter()
        .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == node_id)
        .map(|e| e.from_node_id.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Depth of every node: 0 with no blockers, else 1 + the deepest blocker.
/// A cycle (which the Gate refuses) is cut rather than looping.
pub fn depths(dag: &DagState) -> HashMap<String, u32> {
    fn visit(dag: &DagState, id: &str, memo: &mut HashMap<String, u32>, stack: &mut HashSet<String>) -> u32 {
        if let Some(d) = memo.get(id) {
            return *d;
        }
        if !stack.insert(id.to_string()) {
            return 0;
        }
        let d = blockers(dag, id)
            .iter()
            .filter(|b| dag.nodes.contains_key(*b))
            .map(|b| visit(dag, b, memo, stack) + 1)
            .max()
            .unwrap_or(0);
        stack.remove(id);
        memo.insert(id.to_string(), d);
        d
    }
    let mut memo = HashMap::new();
    for id in dag.nodes.keys() {
        visit(dag, id, &mut memo, &mut HashSet::new());
    }
    memo
}

fn binding_type(node: &DagNode) -> Option<String> {
    // `ao:<harness>` runs as a terminal pane; `bot:` may be hosted or a
    // terminal binding, which the DAG does not record.
    node.executor
        .as_deref()
        .filter(|e| e.starts_with("ao:"))
        .map(|_| "terminal".to_string())
}

/// Card for one node of a projected DAG. `events` are the DAG's events.
pub fn node_card(
    root: &Path,
    events: &[AllternitEvent],
    dag: &DagState,
    node: &DagNode,
    depth: u32,
) -> NodeCard {
    let blocked_by = blockers(dag, &node.node_id);
    let deps_done = blocked_by
        .iter()
        .all(|b| dag.nodes.get(b).is_some_and(|n| n.status == "DONE"));
    let now = chrono::Utc::now();
    let terminal = matches!(
        node.status.as_str(),
        "DONE" | "PASS" | "COMPLETED" | "FAILED" | "FAIL" | "CANCELLED"
    );
    let manual_open = !terminal
        && node
            .wait_gates
            .iter()
            .any(|g| g.kind == WaitGateKind::Manual && !g.is_resolved_ok());
    let other_gate_open = node
        .blocking_wait_gates(now)
        .iter()
        .any(|g| g.kind != WaitGateKind::Manual);

    let (status, gate) = match node.status.as_str() {
        "DONE" | "PASS" | "COMPLETED" => (CardStatus::Done, None),
        "FAILED" | "FAIL" | "CANCELLED" => (CardStatus::Failed, None),
        s if s == judge_status::NEEDS_HUMAN => {
            (CardStatus::NeedsYou, Some(CardGate { kind: "judge".to_string() }))
        }
        _ if manual_open => (CardStatus::NeedsYou, Some(CardGate { kind: "manual".to_string() })),
        s if s == judge_status::VERIFYING => (CardStatus::Checking, None),
        // EXCEPTION: the judge sent it back for another attempt.
        "IN_PROGRESS" | "EXCEPTION" => (CardStatus::Working, None),
        _ if !deps_done || other_gate_open => (CardStatus::Blocked, None),
        "READY" => (CardStatus::Ready, None),
        _ => (CardStatus::New, None),
    };

    let contract = node_folder::read_spec(root, &dag.dag_id, &node.node_id)
        .map(|s| s.proof_contract)
        .unwrap_or_default();
    let (proven, total) = proof::proof_count(events, &dag.dag_id, &node.node_id, &contract);

    NodeCard {
        dag_id: dag.dag_id.clone(),
        node_id: node.node_id.clone(),
        title: node.title.clone(),
        needs_you: status == CardStatus::NeedsYou,
        status,
        assignee: node.assignee.clone(),
        binding_type: binding_type(node),
        proof: ProofTally { proven, total },
        blocked_by,
        depth,
        gate,
    }
}

/// Cards for the work nodes of one DAG (umbrella nodes, i.e. nodes that are
/// some other node's parent, are left out), sorted by depth then id.
pub fn cards_for_dag(root: &Path, events: &[AllternitEvent], dag_id: &str) -> Vec<NodeCard> {
    let evs = dag_events(events, dag_id);
    let dag = project_dag(&evs, dag_id);
    let parents: HashSet<&str> = dag
        .nodes
        .values()
        .filter_map(|n| n.parent_node_id.as_deref())
        .collect();
    let depth = depths(&dag);
    let mut cards: Vec<NodeCard> = dag
        .nodes
        .values()
        .filter(|n| !parents.contains(n.node_id.as_str()))
        .map(|n| node_card(root, &evs, &dag, n, depth.get(&n.node_id).copied().unwrap_or(0)))
        .collect();
    cards.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.node_id.cmp(&b.node_id)));
    cards
}

/// DAG ids that have a `DagCreated` event, in ledger order.
pub fn dag_ids(events: &[AllternitEvent]) -> Vec<String> {
    let mut seen = HashSet::new();
    events
        .iter()
        .filter(|e| e.r#type == "DagCreated")
        .filter_map(|e| e.payload.get("dag_id").and_then(|v| v.as_str()))
        .filter(|d| seen.insert(d.to_string()))
        .map(str::to_string)
        .collect()
}

/// A bot's own nodes across DAGs (`GET /api/factory/nodes?assignee=`).
/// A node is the bot's when its assignee is the address (`slug@team`), the
/// bare slug or `bot:<slug>` (what a pickup for the bot records), or its
/// executor is `bot:<slug>`. `open_only` drops done and failed nodes.
pub fn cards_for_assignee(
    root: &Path,
    events: &[AllternitEvent],
    assignee: &str,
    open_only: bool,
) -> Vec<NodeCard> {
    let slug = assignee.split('@').next().unwrap_or(assignee);
    let bot = format!("bot:{slug}");
    let names = [assignee, slug, bot.as_str()];
    dag_ids(events)
        .iter()
        .flat_map(|d| {
            let dag = project_dag(&dag_events(events, d), d);
            cards_for_dag(root, events, d)
                .into_iter()
                .filter(|c| {
                    c.assignee.as_deref().is_some_and(|a| names.contains(&a))
                        || dag.nodes.get(&c.node_id).and_then(|n| n.executor.as_deref()) == Some(bot.as_str())
                })
                .collect::<Vec<_>>()
        })
        .filter(|c| !open_only || !matches!(c.status, CardStatus::Done | CardStatus::Failed))
        .collect()
}

fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    line.chars().take(120).collect()
}

/// Board for a campaign (Project = Campaign). A campaign's DAG is its
/// `dag_id`. An id that names no campaign but names a DAG gets that DAG's
/// board (CLI-planned DAGs have no campaign).
pub fn build(root: &Path, events: &[AllternitEvent], campaign_id: &str) -> Result<Board> {
    let campaigns = project_campaigns(events);
    let (campaign, dags) = if let Some(c) = campaigns.get(campaign_id) {
        (
            BoardCampaign {
                id: c.campaign_id.clone(),
                title: first_line(&c.objective),
                intent: c.objective.clone(),
            },
            c.dag_id.iter().cloned().collect::<Vec<_>>(),
        )
    } else if dag_ids(events).iter().any(|d| d == campaign_id) {
        let dag = project_dag(&dag_events(events, campaign_id), campaign_id);
        let mut roots: Vec<&DagNode> = dag.nodes.values().filter(|n| n.parent_node_id.is_none()).collect();
        roots.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.node_id.cmp(&b.node_id)));
        let title = roots.first().map(|n| n.title.clone()).unwrap_or_else(|| campaign_id.to_string());
        let intent = node_folder::plan_raw_text(events, campaign_id)
            .or_else(|| {
                roots
                    .first()
                    .and_then(|n| node_folder::read_spec(root, campaign_id, &n.node_id))
                    .and_then(|s| s.intent)
            })
            .unwrap_or_else(|| title.clone());
        (
            BoardCampaign { id: campaign_id.to_string(), title, intent },
            vec![campaign_id.to_string()],
        )
    } else {
        return Err(anyhow!("no campaign or DAG {campaign_id:?}"));
    };

    let cards: Vec<NodeCard> = dags.iter().flat_map(|d| cards_for_dag(root, events, d)).collect();
    let mut by_depth: BTreeMap<u32, Vec<NodeCard>> = BTreeMap::new();
    for c in &cards {
        by_depth.entry(c.depth).or_default().push(c.clone());
    }
    let pick = |f: &dyn Fn(&NodeCard) -> bool| cards.iter().filter(|c| f(c)).cloned().collect::<Vec<_>>();
    let summary = BoardSummary {
        now: pick(&|c| matches!(c.status, CardStatus::Working | CardStatus::Checking)),
        next: pick(&|c| matches!(c.status, CardStatus::Ready | CardStatus::New)),
        proven: ProvenKN {
            k: cards.iter().map(|c| c.proof.proven).sum(),
            n: cards.iter().map(|c| c.proof.total).sum(),
        },
        needs_you: pick(&|c| c.needs_you),
    };
    Ok(Board {
        campaign,
        summary,
        waves: by_depth.into_iter().map(|(depth, nodes)| Wave { depth, nodes }).collect(),
    })
}
