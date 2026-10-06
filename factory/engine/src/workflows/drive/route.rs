//! `on_fail` route-back for `drive` (bounded: determinism contract rule 10).
//!
//! A template step with `on_fail: <step>` mints a node carrying the labels
//! `on_fail:<target node id>` and `max_rounds:<n>`. When that node closes
//! failed (status `FAILED` / `FAIL` — a harness failure or a FAILED close —
//! or `EXCEPTION` — the judge said not accomplished), `drive`:
//!
//! * **routes back** while rounds remain: one Gate 0 refine that reopens the
//!   target and every node between it and the failed node
//!   (`DagNodeStatusChanged` to `NEW`, or `READY` from `EXCEPTION` /
//!   `NEEDS_HUMAN`), appends the failure (status + output reference) to the
//!   target's description, and sets the failed node's state
//!   `on_fail_rounds` to the round number. A `DriveRouteBack` event follows.
//! * **stops** once `on_fail_rounds >= max_rounds`: sets the plan root's
//!   `closure` state to `degraded` (reason = the template's
//!   `closure_degraded` text), reopens the failed node, and puts a manual
//!   needs-you wait-gate (reason `rounds_exhausted`) on it, so nothing runs
//!   it again until a person decides. A `DriveRoundsExhausted` event follows.
//!
//! Rounds are read from the projected DAG (`on_fail_rounds` state), so the
//! count is derived from the ledger with no side database.

use std::collections::{BTreeSet, HashMap};

use crate::templates::{
    CLOSURE_DEGRADED_STATE, DEFAULT_MAX_ROUNDS, MAX_ROUNDS_LABEL_PREFIX, ON_FAIL_LABEL_PREFIX,
};
use crate::work::types::{DagNode, DagState};

/// Ledger event: a failed node was routed back to its `on_fail` target.
pub const ROUTE_BACK: &str = "DriveRouteBack";
/// Ledger event: a failed node used all its rounds; the flow is degraded.
pub const ROUNDS_EXHAUSTED: &str = "DriveRoundsExhausted";
/// Node state dimension: route-back rounds used by this (failing) node.
pub const ROUNDS_STATE: &str = "on_fail_rounds";
/// Needs-you reason on the gate drive adds when rounds run out.
pub const ROUNDS_EXHAUSTED_REASON: &str = "rounds_exhausted";
/// Root `closure` value written when rounds run out.
pub const CLOSURE_DEGRADED: &str = "degraded";

/// One status change of a route: `(node_id, from, to)`.
pub type Reopen = (String, String, String);

/// What drive does about one failed `on_fail` node this pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailRoute {
    /// Reopen `reopen` (target first) and record round `round` of `max_rounds`.
    RouteBack {
        node_id: String,
        target: String,
        round: u32,
        max_rounds: u32,
        reopen: Vec<Reopen>,
    },
    /// Rounds used up: degraded closure + needs-you gate on `node_id`.
    Exhausted {
        node_id: String,
        target: String,
        max_rounds: u32,
        reopen: Reopen,
        root_id: Option<String>,
        closure_text: String,
    },
    /// Cannot route now (a node on the path is held or in a state drive
    /// does not reopen). Reported, re-evaluated next pass.
    Blocked { node_id: String, target: String, why: String },
}

impl FailRoute {
    pub fn node_id(&self) -> &str {
        match self {
            FailRoute::RouteBack { node_id, .. }
            | FailRoute::Exhausted { node_id, .. }
            | FailRoute::Blocked { node_id, .. } => node_id,
        }
    }

    /// The `--dry-run` / status line.
    pub fn line(&self, verb_would: bool) -> String {
        let w = if verb_would { "would route back" } else { "routed back" };
        match self {
            FailRoute::RouteBack { node_id, target, round, max_rounds, reopen } => format!(
                "{w} {node_id} → {target} (round {round}/{max_rounds}; reopen {})",
                reopen.iter().map(|(n, f, t)| format!("{n} {f}→{t}")).collect::<Vec<_>>().join(", ")
            ),
            FailRoute::Exhausted { node_id, target, max_rounds, .. } => format!(
                "{node_id}: rounds exhausted ({max_rounds}/{max_rounds} back to {target}) → degraded{}",
                if verb_would { " (would add a needs-you gate)" } else { "" }
            ),
            FailRoute::Blocked { node_id, target, why } => {
                format!("{node_id}: on_fail → {target} waiting: {why}")
            }
        }
    }
}

/// Statuses that count as "closed failed" for `on_fail`.
pub fn is_failed_status(status: &str) -> bool {
    matches!(status, "FAILED" | "FAIL" | "EXCEPTION")
}

/// `on_fail` target node id from the node's labels.
pub fn on_fail_target(node: &DagNode) -> Option<&str> {
    node.labels.iter().find_map(|l| l.strip_prefix(ON_FAIL_LABEL_PREFIX))
}

/// `max_rounds` from the node's labels (default [`DEFAULT_MAX_ROUNDS`]).
pub fn max_rounds(node: &DagNode) -> u32 {
    node.labels
        .iter()
        .find_map(|l| l.strip_prefix(MAX_ROUNDS_LABEL_PREFIX))
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_ROUNDS)
}

/// Route-back rounds this node has used.
pub fn rounds_used(node: &DagNode) -> u32 {
    node.state.get(ROUNDS_STATE).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Status a node is reopened to: `Some(None)` = already open,
/// `None` = drive must not reopen it (held / mid-run).
fn reopen_status(node: &DagNode) -> Option<Option<&'static str>> {
    if node.current_wih_id.is_some() {
        return None;
    }
    match node.status.as_str() {
        "NEW" | "READY" => Some(None),
        "DONE" | "PASS" | "COMPLETED" | "FAILED" | "FAIL" | "CANCELLED" => Some(Some("NEW")),
        "EXCEPTION" | "NEEDS_HUMAN" => Some(Some("READY")),
        _ => None,
    }
}

/// Nodes on a `blocked_by` path from `target` to `node_id`, both included.
fn between(dag: &DagState, target: &str, node_id: &str) -> BTreeSet<String> {
    let mut succ: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut pred: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in dag.edges.iter().filter(|e| e.edge_type == "blocked_by") {
        succ.entry(e.from_node_id.as_str()).or_default().push(e.to_node_id.as_str());
        pred.entry(e.to_node_id.as_str()).or_default().push(e.from_node_id.as_str());
    }
    let walk = |start: &str, next: &HashMap<&str, Vec<&str>>| {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start.to_string()];
        while let Some(n) = stack.pop() {
            if seen.insert(n.clone()) {
                for m in next.get(n.as_str()).into_iter().flatten() {
                    stack.push(m.to_string());
                }
            }
        }
        seen
    };
    let down = walk(target, &succ);
    let up = walk(node_id, &pred);
    down.intersection(&up).cloned().collect()
}

/// The root of `node`: follow `parent_node_id` to the top.
fn root_of(dag: &DagState, node: &DagNode) -> Option<String> {
    let mut current = node.parent_node_id.clone()?;
    for _ in 0..dag.nodes.len() {
        match dag.nodes.get(&current).and_then(|n| n.parent_node_id.clone()) {
            Some(p) => current = p,
            None => return Some(current),
        }
    }
    Some(current)
}

/// Pure decision for every failed node carrying `on_fail`, sorted by node id.
pub fn plan_fail_routes(dag: &DagState) -> Vec<FailRoute> {
    let mut nodes: Vec<&DagNode> = dag
        .nodes
        .values()
        .filter(|n| is_failed_status(&n.status) && n.current_wih_id.is_none())
        .filter(|n| on_fail_target(n).is_some())
        .collect();
    nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let mut out = Vec::new();
    for node in nodes {
        let target = on_fail_target(node).unwrap_or_default().to_string();
        let blocked = |why: String| FailRoute::Blocked {
            node_id: node.node_id.clone(),
            target: target.clone(),
            why,
        };
        if !dag.nodes.contains_key(&target) {
            out.push(blocked(format!("on_fail target {target} is not in this dag")));
            continue;
        }
        let limit = max_rounds(node);
        let used = rounds_used(node);
        let own_to = reopen_status(node).flatten().unwrap_or("NEW");
        if used >= limit {
            let root_id = root_of(dag, node);
            let closure_text = root_id
                .as_ref()
                .and_then(|r| dag.nodes.get(r))
                .and_then(|r| r.state.get(CLOSURE_DEGRADED_STATE).cloned())
                .unwrap_or_else(|| format!("stopped after {limit} route-back round(s)"));
            out.push(FailRoute::Exhausted {
                node_id: node.node_id.clone(),
                target,
                max_rounds: limit,
                reopen: (node.node_id.clone(), node.status.clone(), own_to.to_string()),
                root_id,
                closure_text,
            });
            continue;
        }
        let path = between(dag, &target, &node.node_id);
        if !path.contains(&target) || !path.contains(&node.node_id) {
            out.push(blocked(format!("{target} is not a blocked_by predecessor of {}", node.node_id)));
            continue;
        }
        let mut reopen = Vec::new();
        let mut held = None;
        // Target first, then the rest in id order: stable output.
        let ordered = std::iter::once(&target).chain(path.iter().filter(|n| **n != target));
        for id in ordered {
            let n = &dag.nodes[id];
            match reopen_status(n) {
                Some(Some(to)) => reopen.push((id.clone(), n.status.clone(), to.to_string())),
                Some(None) => {}
                None => {
                    held = Some(format!("{id} is {}{}", n.status, n
                        .current_wih_id
                        .as_deref()
                        .map(|w| format!(" (held by {w})"))
                        .unwrap_or_default()));
                    break;
                }
            }
        }
        if let Some(why) = held {
            out.push(blocked(why));
            continue;
        }
        out.push(FailRoute::RouteBack {
            node_id: node.node_id.clone(),
            target,
            round: used + 1,
            max_rounds: limit,
            reopen,
        });
    }
    out
}

/// Text appended to the reopened target's description for round `round`.
pub fn feedback_text(failed: &DagNode, round: u32, max_rounds: u32) -> String {
    let output = match &failed.output {
        Some(o) => format!(
            "Its output (what failed and why): {} (receipt {}).",
            o.output_path, o.receipt_id
        ),
        None => "It recorded no output; check its WIH and the drive run log.".to_string(),
    };
    format!(
        "\n\n---\nRound {round} of {max_rounds}: `{}` (\"{}\") closed {} and routed back here. {output} \
         Fix what it reports, then finish this step again.",
        failed.node_id, failed.title, failed.status
    )
}
