//! "Needs you" projection for node-scoped wait-gates.
//!
//! An unresolved Manual wait-gate on a DAG node is work waiting on a human.
//! This projection lists them so surfaces (API visibility `needsYou`, CLI)
//! can show them without re-deriving gate state.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::core::types::AllternitEvent;
use crate::wait_gates::WaitGateKind;
use crate::work::projection::project_dag;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PendingManualGate {
    pub dag_id: String,
    pub node_id: String,
    pub node_title: String,
    pub gate_id: String,
    pub description: String,
    pub created_at: String,
    /// True when every blocked_by predecessor is DONE, i.e. the gate is the
    /// only thing between the node and READY.
    pub deps_done: bool,
}

/// Unresolved (or failed) Manual wait-gates on non-terminal nodes across all
/// DAGs in `events`, oldest first.
pub fn pending_manual_gates(events: &[AllternitEvent]) -> Vec<PendingManualGate> {
    let mut dag_ids = BTreeSet::new();
    for evt in events {
        if evt.r#type == "DagNodeWaitGateAdded" {
            if let Some(d) = evt.payload.get("dag_id").and_then(|v| v.as_str()) {
                dag_ids.insert(d.to_string());
            }
        }
    }
    let mut out = Vec::new();
    for dag_id in dag_ids {
        let dag_events: Vec<AllternitEvent> = events
            .iter()
            .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str()))
            .cloned()
            .collect();
        let dag = project_dag(&dag_events, &dag_id);
        for node in dag.nodes.values() {
            if node.status == "DONE" || node.status == "FAILED" {
                continue;
            }
            let deps_done = dag
                .edges
                .iter()
                .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == node.node_id)
                .all(|e| {
                    dag.nodes
                        .get(&e.from_node_id)
                        .is_some_and(|n| n.status == "DONE")
                });
            for gate in &node.wait_gates {
                if gate.kind != WaitGateKind::Manual || gate.is_resolved_ok() {
                    continue;
                }
                out.push(PendingManualGate {
                    dag_id: dag_id.clone(),
                    node_id: node.node_id.clone(),
                    node_title: node.title.clone(),
                    gate_id: gate.gate_id.clone(),
                    description: gate.description.clone(),
                    created_at: gate.created_at.clone(),
                    deps_done,
                });
            }
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.gate_id.cmp(&b.gate_id)));
    out
}
