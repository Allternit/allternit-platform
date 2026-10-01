//! Scripted runner for task-type graphs (eval harness; no model, no effects).
//!
//! Walks an instantiated graph the way the executor walks BUG_FIX: every
//! visited node is routed by the commrails [`Router`] over the scripted pool
//! (`executor::scripted_pool`, so routing and capabilities are exercised for
//! real) and walks the kernel node lifecycle to `Committed` or `Failed`.
//! Completion is verifier-owned: the run completes only when the graph's
//! `gate` node finds receipt-backed evidence for every blocking criterion of
//! the task type's completion contract (`missing_evidence`).
//!
//! The script decides only what a real executor would observe: which nodes
//! fail, which WAIT gates open, and which verifier evidence is withheld.

use super::{evidence_of, instantiate, require};
use crate::agency_api::executor::scripted_pool;
use allternit_commrails::judge::completion::missing_evidence;
use allternit_commrails::kernel::graph::ComputeGraph;
use allternit_commrails::kernel::lifecycle::{try_close, try_transition};
use allternit_commrails::kernel::router::{BudgetLedger, ExecutionPlan, Router, RouterConfig};
use allternit_commrails::kernel::{CloseOutcome, NodeState};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const GATE_PRIMITIVE: &str = "ver.check_acceptance_evidence";
const MAX_STEPS: usize = 64;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Script {
    /// Nodes whose attempt fails.
    #[serde(default)]
    pub fail: HashSet<String>,
    /// WAIT gates: `"open"` lets the run through, `"closed"` rejects; absent parks the run.
    #[serde(default)]
    pub wait: HashMap<String, String>,
    /// Criteria whose verifier commits without producing an evidence receipt.
    #[serde(default)]
    pub withhold: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Completed,
    Failed,
    /// Parked at a WAIT gate nobody opened.
    Waiting,
    /// The completion gate found missing evidence and asked for it.
    NeedsEvidence,
    /// The graph could not be instantiated safely (fail closed).
    Refused,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Completed => "completed",
            Status::Failed => "failed",
            Status::Waiting => "waiting",
            Status::NeedsEvidence => "needs_evidence",
            Status::Refused => "refused",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: Status,
    pub reason: String,
    /// `(node_id, committed)` in visit order.
    pub visited: Vec<(String, bool)>,
    pub stopped_at: Option<String>,
    pub evidence: Vec<String>,
    pub missing: Vec<String>,
    pub plans: Vec<(String, ExecutionPlan)>,
}

impl Outcome {
    fn new(status: Status, reason: impl Into<String>) -> Self {
        Self { status, reason: reason.into(), visited: vec![], stopped_at: None, evidence: vec![], missing: vec![], plans: vec![] }
    }
    pub fn visited_ids(&self) -> Vec<&str> {
        self.visited.iter().map(|(n, _)| n.as_str()).collect()
    }
}

fn close(id: &str, ok: bool) -> Result<(), String> {
    let mut s = NodeState::Declared;
    for to in [NodeState::Admitted, NodeState::Ready, NodeState::Leased, NodeState::Spawned, NodeState::Running,
               NodeState::OutputReady, NodeState::Verifying] {
        s = try_transition(s, to).map_err(|e| format!("{id}: {e:?}"))?;
    }
    let r = if ok {
        try_transition(s, NodeState::Committed).and_then(|s| try_close(s, CloseOutcome::Committed))
    } else {
        try_transition(s, NodeState::Replan).and_then(|s| try_close(s, CloseOutcome::Failed))
    };
    r.map(|_| ()).map_err(|e| format!("{id}: {e:?}"))
}

fn next(g: &ComputeGraph, from: &str, kind: &str) -> Option<String> {
    g.edges.iter().find(|e| e.from == from && e.kind() == kind).map(|e| e.to.clone())
}

/// Instantiate `task_type` for `goal`/`params` and run it under `script`.
pub fn run_scripted(task_type: &str, goal: &str, params: &Value, script: &Script) -> Outcome {
    let t = match require(task_type) {
        Ok(t) => t,
        Err(e) => return Outcome::new(Status::Refused, e.to_string()),
    };
    let writable: Vec<String> = params["writable_resources"].as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let task_id = format!("task.{}.eval", t.id.to_ascii_lowercase());
    let g = match instantiate(t, &task_id, &writable) {
        Ok(g) => g,
        Err(e) => return Outcome::new(Status::Refused, e.to_string()),
    };
    let Some(policy) = super::completion_policy(t) else {
        return Outcome::new(Status::Refused, format!("{} has no completion contract", t.id));
    };
    let _ = goal; // the scripted executor's outputs don't depend on the goal text

    let pool = scripted_pool(&g);
    let cfg = RouterConfig::default();
    let router = Router::new(&pool, &cfg);
    let ledger = BudgetLedger { remaining_cost_units: 1.0e9, remaining_wall_ms: None };

    let mut out = Outcome::new(Status::Failed, "");
    let mut cur = g.entry_nodes.first().cloned().unwrap_or_default();
    for _ in 0..MAX_STEPS {
        let Some(node) = g.node(&cur) else {
            out.reason = format!("graph has no node {cur}");
            return out;
        };
        let plan = match router.route(node, &ledger) {
            Ok(p) => p,
            Err(e) => {
                out.reason = format!("route {cur}: {e}");
                out.stopped_at = Some(cur);
                return out;
            }
        };
        out.plans.push((cur.clone(), plan));

        // Preconditions: a node's `evidence_required` must already be on file.
        let unmet: Vec<&String> = node.evidence_required.iter()
            .filter(|c| !out.evidence.iter().any(|e| e.starts_with(&format!("{c}:"))))
            .collect();

        let (ok, wait_parked) = if node.primitive_id == GATE_PRIMITIVE && !node.evidence_required.is_empty() {
            out.missing = missing_evidence(&policy, &out.evidence);
            (out.missing.is_empty(), false)
        } else if node.node_kind == "WAIT" {
            match script.wait.get(&cur).map(String::as_str) {
                Some("open") => (unmet.is_empty(), false),
                Some(_) => (false, false),
                None => (false, true),
            }
        } else {
            (unmet.is_empty() && !script.fail.contains(&cur), false)
        };

        if wait_parked {
            out.status = Status::Waiting;
            out.reason = format!("waiting at {cur}");
            out.stopped_at = Some(cur);
            return out;
        }
        if let Err(e) = close(&cur, ok) {
            out.reason = e;
            out.stopped_at = Some(cur);
            return out;
        }
        out.visited.push((cur.clone(), ok));

        if ok {
            if let Some(c) = evidence_of(node) {
                if !script.withhold.contains(c) {
                    out.evidence.push(format!("{c}:receipt:{task_id}:{cur}"));
                }
            }
            match next(&g, &cur, "NORMAL") {
                Some(n) => cur = n,
                None if g.completion_nodes.contains(&cur) => {
                    out.status = Status::Completed;
                    out.reason = "completion contract satisfied".into();
                    out.stopped_at = Some(cur);
                    return out;
                }
                None => {
                    out.reason = format!("{cur} has no successor and is not a completion node");
                    out.stopped_at = Some(cur);
                    return out;
                }
            }
            continue;
        }

        // Failure: follow the node's FAILURE edge per its on_failure strategy.
        let strategy = node.on_failure.strategy.clone();
        let is_gate = node.primitive_id == GATE_PRIMITIVE;
        match next(&g, &cur, "FAILURE") {
            Some(target) if matches!(strategy.as_str(), "FALLBACK" | "ESCALATE") && !is_gate => cur = target,
            Some(target) => {
                // ROLLBACK, or the completion gate asking for evidence: run the
                // target, then stop.
                let committed = close(&target, true).is_ok();
                out.visited.push((target.clone(), committed));
                out.status = if is_gate { Status::NeedsEvidence } else { Status::Failed };
                out.reason = if is_gate { format!("missing evidence: {}", out.missing.join(", ")) } else { format!("{cur} failed ({strategy})") };
                out.stopped_at = Some(target);
                return out;
            }
            None => {
                out.reason = if unmet.is_empty() { format!("{cur} failed") } else { format!("{cur} preconditions unmet: {unmet:?}") };
                out.stopped_at = Some(cur);
                return out;
            }
        }
    }
    out.reason = "step cap reached".into();
    out
}
