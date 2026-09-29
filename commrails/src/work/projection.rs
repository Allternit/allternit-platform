use std::collections::HashMap;

use serde_json::Value;

use crate::core::types::AllternitEvent;
use crate::wait_gates::WaitGateKind;
use crate::work::types::{DagEdge, DagNode, DagRelation, DagState, NodeOutputRef, NodeWaitGate};

pub fn project_dag(events: &[AllternitEvent], dag_id: &str) -> DagState {
    let mut dag = DagState {
        dag_id: dag_id.to_string(),
        nodes: HashMap::new(),
        edges: Vec::new(),
        relations: Vec::new(),
    };

    // wih_id -> (dag_id, node_id) from WIHCreated events. Pre-v5 ledgers carry
    // pickup/close events without dag_id/node_id in the payload; this map lets
    // the WIH arms below resolve them. NOTE: callers that pre-filter events by
    // payload.dag_id (e.g. Gate::events_for_dag) drop dag_id-less WIH events
    // before project_dag sees them, so for those callers the fallback only
    // works once ledgers contain enriched (post-v5) events.
    let mut wih_nodes: HashMap<String, (String, String)> = HashMap::new();

    for evt in events {
        if evt.r#type == "WIHCreated" {
            if let (Some(wih_id), Some(e_dag_id), Some(node_id)) = (
                get_str(&evt.payload, "wih_id"),
                get_str(&evt.payload, "dag_id"),
                get_str(&evt.payload, "node_id"),
            ) {
                wih_nodes.insert(wih_id, (e_dag_id, node_id));
            }
        }
        match evt.r#type.as_str() {
            "DagNodeCreated" => {
                if let Some(node) = parse_node_created(evt, dag_id) {
                    dag.nodes.insert(node.node_id.clone(), node);
                }
            }
            "DagNodeUpdated" => {
                // Older DagNodeUpdated events carry no dag_id; newer ones do,
                // and must not leak onto a same-named node in another dag.
                if let Some(e_dag) = get_str(&evt.payload, "dag_id") {
                    if e_dag != dag_id {
                        continue;
                    }
                }
                if let Some(node_id) = get_str(&evt.payload, "node_id") {
                    if let Some(node) = dag.nodes.get_mut(&node_id) {
                        if let Some(patch) = evt.payload.get("patch") {
                            apply_node_patch(node, patch);
                            node.updated_at = Some(evt.ts.clone());
                        }
                    }
                }
            }
            "DagNodeRemoved" => {
                if get_str(&evt.payload, "dag_id").as_deref() == Some(dag_id) {
                    if let Some(node_id) = get_str(&evt.payload, "node_id") {
                        dag.nodes.remove(&node_id);
                        dag.edges
                            .retain(|e| e.from_node_id != node_id && e.to_node_id != node_id);
                    }
                }
            }
            "DagNodeReparented" => {
                if get_str(&evt.payload, "dag_id").as_deref() == Some(dag_id) {
                    if let Some(node_id) = get_str(&evt.payload, "node_id") {
                        if let Some(node) = dag.nodes.get_mut(&node_id) {
                            node.parent_node_id = evt
                                .payload
                                .get("new_parent_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            node.updated_at = Some(evt.ts.clone());
                        }
                    }
                }
            }
            "DagEdgeAdded" => {
                if let Some(edge) = parse_edge_added(&evt.payload) {
                    dag.edges.push(edge);
                }
            }
            "DagRelationAdded" => {
                if let Some(rel) = parse_relation_added(&evt.payload) {
                    dag.relations.push(rel);
                }
            }
            "DagNodeStatusChanged" => {
                if let Some(node_id) = get_str(&evt.payload, "node_id") {
                    if let Some(node) = dag.nodes.get_mut(&node_id) {
                        if let Some(to) = get_str(&evt.payload, "to") {
                            node.status = to;
                            node.updated_at = Some(evt.ts.clone());
                        }
                    }
                }
            }
            "LabelAdded" => {
                if let Some(node_id) = get_str(&evt.payload, "node_id") {
                    if let Some(node) = dag.nodes.get_mut(&node_id) {
                        if let Some(label) = get_str(&evt.payload, "label") {
                            if !node.labels.contains(&label) {
                                node.labels.push(label);
                                node.updated_at = Some(evt.ts.clone());
                            }
                        }
                    }
                }
            }
            "LabelRemoved" => {
                if let Some(node_id) = get_str(&evt.payload, "node_id") {
                    if let Some(node) = dag.nodes.get_mut(&node_id) {
                        if let Some(label) = get_str(&evt.payload, "label") {
                            node.labels.retain(|l| l != &label);
                            node.updated_at = Some(evt.ts.clone());
                        }
                    }
                }
            }
            "StateSet" => {
                if let Some(node_id) = get_str(&evt.payload, "node_id") {
                    if let Some(node) = dag.nodes.get_mut(&node_id) {
                        if let (Some(dim), Some(value)) = (
                            get_str(&evt.payload, "dimension"),
                            get_str(&evt.payload, "value"),
                        ) {
                            node.state.insert(dim, value);
                            node.updated_at = Some(evt.ts.clone());
                        }
                    }
                }
            }
            "DagNodeOutputRecorded" => {
                if get_str(&evt.payload, "dag_id").as_deref() == Some(dag_id) {
                    if let Some(node_id) = get_str(&evt.payload, "node_id") {
                        if let Some(node) = dag.nodes.get_mut(&node_id) {
                            if let Some(output) = parse_output_recorded(evt) {
                                node.output = Some(output);
                                node.updated_at = Some(evt.ts.clone());
                            }
                        }
                    }
                }
            }
            "DagNodeWaitGateAdded" => {
                if get_str(&evt.payload, "dag_id").as_deref() == Some(dag_id) {
                    if let Some(node_id) = get_str(&evt.payload, "node_id") {
                        if let Some(node) = dag.nodes.get_mut(&node_id) {
                            if let Some(gate) = parse_wait_gate_added(evt) {
                                node.wait_gates.retain(|g| g.gate_id != gate.gate_id);
                                node.wait_gates.push(gate);
                                node.updated_at = Some(evt.ts.clone());
                            }
                        }
                    }
                }
            }
            "DagNodeWaitGateResolved" => {
                if get_str(&evt.payload, "dag_id").as_deref() == Some(dag_id) {
                    if let (Some(node_id), Some(gate_id)) = (
                        get_str(&evt.payload, "node_id"),
                        get_str(&evt.payload, "gate_id"),
                    ) {
                        if let Some(node) = dag.nodes.get_mut(&node_id) {
                            if let Some(gate) =
                                node.wait_gates.iter_mut().find(|g| g.gate_id == gate_id)
                            {
                                gate.outcome = evt
                                    .payload
                                    .get("outcome")
                                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                                gate.resolved_at = Some(evt.ts.clone());
                                gate.resolved_by = get_str(&evt.payload, "resolved_by")
                                    .or_else(|| Some(evt.actor.id.clone()));
                                gate.reason = get_str(&evt.payload, "reason");
                                node.updated_at = Some(evt.ts.clone());
                            }
                        }
                    }
                }
            }
            "WIHPickedUp" => {
                // See the wih_nodes note above: dag_id-less pickup events are
                // invisible to callers that pre-filter by payload.dag_id.
                let wih_id = match get_str(&evt.payload, "wih_id") {
                    Some(id) => id,
                    None => continue,
                };
                let (e_dag_id, node_id) = match (
                    get_str(&evt.payload, "dag_id"),
                    get_str(&evt.payload, "node_id"),
                ) {
                    (Some(d), Some(n)) => (d, n),
                    _ => match wih_nodes.get(&wih_id) {
                        Some((d, n)) => (d.clone(), n.clone()),
                        None => continue,
                    },
                };
                if e_dag_id != dag_id {
                    continue;
                }
                if let Some(node) = dag.nodes.get_mut(&node_id) {
                    node.current_wih_id = Some(wih_id);
                    if let Some(agent_id) = get_str(&evt.payload, "agent_id") {
                        node.assignee = Some(agent_id);
                    }
                    node.updated_at = Some(evt.ts.clone());
                }
            }
            "WIHClosedSigned" | "WIHArchived" => {
                // Semantics: closing/archiving a WIH releases the node — clear
                // both current_wih_id and assignee.
                let wih_id = match get_str(&evt.payload, "wih_id") {
                    Some(id) => id,
                    None => continue,
                };
                let (e_dag_id, node_id) = match (
                    get_str(&evt.payload, "dag_id"),
                    get_str(&evt.payload, "node_id"),
                ) {
                    (Some(d), Some(n)) => (d, n),
                    _ => match wih_nodes.get(&wih_id) {
                        Some((d, n)) => (d.clone(), n.clone()),
                        None => continue,
                    },
                };
                if e_dag_id != dag_id {
                    continue;
                }
                if let Some(node) = dag.nodes.get_mut(&node_id) {
                    node.current_wih_id = None;
                    node.assignee = None;
                    node.updated_at = Some(evt.ts.clone());
                }
            }
            _ => {}
        }
    }

    apply_readiness(&mut dag);
    dag
}

fn parse_node_created(evt: &AllternitEvent, dag_id: &str) -> Option<DagNode> {
    let payload = &evt.payload;
    let payload_dag = get_str(payload, "dag_id")?;
    if payload_dag != dag_id {
        return None;
    }
    let node_id = get_str(payload, "node_id")?;
    let node_kind = get_str(payload, "node_kind").unwrap_or_else(|| "task".to_string());
    let title = get_str(payload, "title").unwrap_or_else(|| "Untitled".to_string());
    let parent_node_id = payload
        .get("parent_node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = payload
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let execution_mode = get_str(payload, "execution_mode").unwrap_or_else(|| "shared".to_string());
    let owner_role = payload
        .get("owner_role")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let priority = payload.get("priority").and_then(|v| v.as_i64());
    let labels = payload
        .get("labels")
        .or_else(|| payload.get("tags"))
        .and_then(|v| v.as_array())
        .map(|arr| extract_string_vec(arr))
        .unwrap_or_default();
    let assignee = payload
        .get("assignee")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let spec_id = payload
        .get("spec_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let notes = payload
        .get("notes")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let acceptance = payload
        .get("acceptance")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let design = payload
        .get("design")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let executor = payload
        .get("executor")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(DagNode {
        node_id,
        dag_id: payload_dag,
        parent_node_id,
        node_kind,
        title,
        description,
        execution_mode,
        owner_role,
        priority,
        labels,
        status: "NEW".to_string(),
        current_wih_id: None,
        assignee,
        spec_id,
        notes,
        acceptance,
        design,
        state: HashMap::new(),
        created_at: Some(evt.ts.clone()),
        updated_at: Some(evt.ts.clone()),
        worktree: None,
        executor,
        output: None,
        wait_gates: Vec::new(),
    })
}

fn parse_output_recorded(evt: &AllternitEvent) -> Option<NodeOutputRef> {
    let p = &evt.payload;
    Some(NodeOutputRef {
        wih_id: get_str(p, "wih_id").unwrap_or_default(),
        receipt_id: get_str(p, "receipt_id")?,
        blob_id: get_str(p, "blob_id")?,
        sha256: get_str(p, "sha256").unwrap_or_default(),
        size_bytes: p.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
        output_path: get_str(p, "output_path")?,
        recorded_at: evt.ts.clone(),
    })
}

fn parse_wait_gate_added(evt: &AllternitEvent) -> Option<NodeWaitGate> {
    let p = &evt.payload;
    let kind: WaitGateKind = serde_json::from_value(p.get("kind")?.clone()).ok()?;
    let params = p
        .get("params")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    Some(NodeWaitGate {
        gate_id: get_str(p, "gate_id")?,
        description: get_str(p, "description").unwrap_or_else(|| format!("{kind} gate")),
        kind,
        params,
        created_at: evt.ts.clone(),
        outcome: None,
        resolved_at: None,
        resolved_by: None,
        reason: None,
    })
}

fn parse_edge_added(payload: &Value) -> Option<DagEdge> {
    Some(DagEdge {
        from_node_id: get_str(payload, "from_node_id")?,
        to_node_id: get_str(payload, "to_node_id")?,
        edge_type: get_str(payload, "edge_type").unwrap_or_else(|| "blocked_by".to_string()),
    })
}

fn parse_relation_added(payload: &Value) -> Option<DagRelation> {
    let context_share = payload
        .get("context_share")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(DagRelation {
        a: get_str(payload, "a")?,
        b: get_str(payload, "b")?,
        note: payload
            .get("note")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        context_share,
    })
}

fn apply_node_patch(node: &mut DagNode, patch: &Value) {
    if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
        node.title = title.to_string();
    }
    if let Some(kind) = patch.get("node_kind").and_then(|v| v.as_str()) {
        node.node_kind = kind.to_string();
    }
    if let Some(desc) = patch.get("description").and_then(|v| v.as_str()) {
        node.description = Some(desc.to_string());
    }
    if let Some(mode) = patch.get("execution_mode").and_then(|v| v.as_str()) {
        node.execution_mode = mode.to_string();
    }
    if let Some(role) = patch.get("owner_role").and_then(|v| v.as_str()) {
        node.owner_role = Some(role.to_string());
    }
    if let Some(priority) = patch.get("priority").and_then(|v| v.as_i64()) {
        node.priority = Some(priority);
    }
    if let Some(labels) = patch
        .get("labels")
        .or_else(|| patch.get("tags"))
        .and_then(|v| v.as_array())
    {
        node.labels = extract_string_vec(labels);
    }
    if let Some(assignee) = patch.get("assignee").and_then(|v| v.as_str()) {
        node.assignee = Some(assignee.to_string());
    }
    if let Some(spec_id) = patch.get("spec_id").and_then(|v| v.as_str()) {
        node.spec_id = Some(spec_id.to_string());
    }
    if let Some(notes) = patch.get("notes").and_then(|v| v.as_str()) {
        node.notes = Some(notes.to_string());
    }
    if let Some(acceptance) = patch.get("acceptance").and_then(|v| v.as_str()) {
        node.acceptance = Some(acceptance.to_string());
    }
    if let Some(design) = patch.get("design").and_then(|v| v.as_str()) {
        node.design = Some(design.to_string());
    }
    if let Some(executor) = patch.get("executor").and_then(|v| v.as_str()) {
        node.executor = Some(executor.to_string());
    }
}

fn apply_readiness(dag: &mut DagState) {
    let edges = dag.edges.clone();
    let status_map: std::collections::HashMap<String, String> = dag
        .nodes
        .iter()
        .map(|(id, node)| (id.clone(), node.status.clone()))
        .collect();

    for (node_id, node) in dag.nodes.iter_mut() {
        if node.status != "NEW" {
            continue;
        }
        let blockers: Vec<&DagEdge> = edges
            .iter()
            .filter(|edge| edge.to_node_id == *node_id && edge.edge_type == "blocked_by")
            .collect();
        let deps_satisfied = blockers.iter().all(|edge| {
            status_map
                .get(&edge.from_node_id)
                .map(|status| status == "DONE")
                .unwrap_or(false)
        });
        // A node with an unresolved wait-gate stays NEW (not ready). Timer
        // gates count as unresolved here until the gate records their
        // resolution; `ready_nodes` evaluates elapsed timers against the clock.
        let gates_resolved = node.wait_gates.iter().all(|g| g.is_resolved_ok());
        if deps_satisfied && gates_resolved {
            node.status = "READY".to_string();
        }
    }
}

fn get_str(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn extract_string_vec(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect()
}
