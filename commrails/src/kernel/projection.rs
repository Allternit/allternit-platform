//! One-way projection: ComputeGraphIR -> WIH DAG (decision Q3).
//!
//! The graph is authoritative. The projection is disposable and
//! deterministic: node_id -> WIH task, NORMAL edges -> `depends_on`.
//! FAILURE and LOOP edges are control flow the DAG cannot express; they are
//! carried as `failure_targets` / `loop_targets` metadata, never as deps.
//! There is deliberately NO function that parses a projection back into a
//! graph.

use serde::{Deserialize, Serialize};

use super::graph::ComputeGraph;
use super::lifecycle::{legacy_status, NodeState};
use crate::service::WihInfo;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WihTask {
    pub wih_id: String,
    pub node_id: String,
    pub primitive_id: String,
    pub node_kind: String,
    pub depends_on: Vec<String>,
    pub failure_targets: Vec<String>,
    pub loop_targets: Vec<String>,
    /// Legacy ledger status string (`NEW` for every freshly projected task;
    /// runtime status lives in the Work Runtime ledger, not the projection).
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WihProjection {
    pub dag_id: String,
    pub tasks: Vec<WihTask>,
}

pub fn wih_id_for(graph_id: &str, node_id: &str) -> String {
    format!("wih-{graph_id}-{node_id}")
}

/// Project a graph. Task order follows node order; dependency lists are
/// sorted so the output is deterministic for equal graphs.
pub fn project(g: &ComputeGraph) -> WihProjection {
    let tasks = g
        .nodes
        .iter()
        .map(|n| {
            let collect = |kind: &str, incoming: bool| {
                let mut v: Vec<String> = g
                    .edges
                    .iter()
                    .filter(|e| e.kind() == kind)
                    .filter(|e| if incoming { e.to == n.node_id } else { e.from == n.node_id })
                    .map(|e| if incoming { e.from.clone() } else { e.to.clone() })
                    .collect();
                v.sort();
                v.dedup();
                v
            };
            WihTask {
                wih_id: wih_id_for(&g.graph_id, &n.node_id),
                node_id: n.node_id.clone(),
                primitive_id: n.primitive_id.clone(),
                node_kind: n.node_kind.clone(),
                depends_on: collect("NORMAL", true)
                    .into_iter()
                    .map(|d| wih_id_for(&g.graph_id, &d))
                    .collect(),
                failure_targets: collect("FAILURE", false),
                loop_targets: collect("LOOP", false),
                status: legacy_status(NodeState::Admitted, None).to_string(),
            }
        })
        .collect();
    WihProjection { dag_id: g.graph_id.clone(), tasks }
}

impl WihProjection {
    /// Adapter to the service-layer listing type.
    pub fn to_wih_infos(&self) -> Vec<WihInfo> {
        self.tasks
            .iter()
            .map(|t| WihInfo {
                wih_id: t.wih_id.clone(),
                node_id: t.node_id.clone(),
                dag_id: Some(self.dag_id.clone()),
                status: t.status.clone(),
                title: Some(t.primitive_id.clone()),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn g() -> ComputeGraph {
        let n = |id: &str| {
            json!({"node_id": id, "primitive_id": "obs.observe_environment", "node_kind": "COMPUTE",
                   "on_failure": {"strategy": "FAIL"}})
        };
        serde_json::from_value(json!({
            "graph_id": "g1", "entry_nodes": ["a"], "completion_nodes": ["c"],
            "nodes": [n("a"), n("b"), n("c")],
            "edges": [
                {"from":"a","to":"b"}, {"from":"b","to":"c"},
                {"from":"a","to":"c","edge_kind":"NORMAL"},
                {"from":"b","to":"a","edge_kind":"FAILURE"},
                {"from":"c","to":"b","edge_kind":"LOOP"}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn edges_become_deps() {
        let p = project(&g());
        assert_eq!(p.dag_id, "g1");
        assert_eq!(p.tasks.len(), 3);
        assert!(p.tasks[0].depends_on.is_empty());
        assert_eq!(p.tasks[1].depends_on, vec!["wih-g1-a"]);
        assert_eq!(p.tasks[2].depends_on, vec!["wih-g1-a", "wih-g1-b"]);
        assert_eq!(p.tasks[1].failure_targets, vec!["a"]);
        assert_eq!(p.tasks[2].loop_targets, vec!["b"]);
        assert!(p.tasks.iter().all(|t| t.status == "NEW"));
    }

    #[test]
    fn projection_is_deterministic_and_json_round_trips() {
        let p1 = project(&g());
        let p2 = project(&g());
        assert_eq!(p1, p2);
        let s = serde_json::to_string(&p1).unwrap();
        let back: WihProjection = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p1);
    }

    #[test]
    fn every_node_projected_and_deps_match_normal_edges() {
        let graph = g();
        let p = project(&graph);
        for n in &graph.nodes {
            assert!(p.tasks.iter().any(|t| t.node_id == n.node_id));
        }
        let dep_count: usize = p.tasks.iter().map(|t| t.depends_on.len()).sum();
        assert_eq!(dep_count, graph.edges.iter().filter(|e| e.kind() == "NORMAL").count());
        // projected deps form a DAG: every dep precedes its task
        for (i, t) in p.tasks.iter().enumerate() {
            for d in &t.depends_on {
                assert!(p.tasks.iter().position(|x| &x.wih_id == d).unwrap() < i);
            }
        }
    }

    #[test]
    fn wih_info_adapter() {
        let infos = project(&g()).to_wih_infos();
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].wih_id, "wih-g1-a");
        assert_eq!(infos[0].dag_id.as_deref(), Some("g1"));
        assert_eq!(infos[0].status, "NEW");
    }

    #[test]
    fn projection_never_feeds_back_as_truth() {
        // Editing a projection cannot change the graph: project is the only
        // direction and takes the graph by shared reference.
        let graph = g();
        let mut p = project(&graph);
        p.tasks.clear();
        assert_eq!(project(&graph).tasks.len(), 3);
    }
}
