//! ComputeGraphIRV1 (ABI 1.0.0) typed view + static validator.
//!
//! The validator enforces the seven graph invariants (ledger CL-164, Living
//! Architecture §25 L2734-2741) plus the structural rules the schema cannot
//! express (unique ids, edge endpoints, registry membership, acyclicity
//! ignoring LOOP edges). It is pure: no I/O, no model calls.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::registry::PrimitiveRegistry;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub must_change: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OnFailure {
    pub strategy: String,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNode {
    pub node_id: String,
    pub primitive_id: String,
    pub node_kind: String,
    #[serde(default)]
    pub cognitive_role: Option<String>,
    #[serde(default)]
    pub capability_request: Option<Value>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub read_set: Vec<String>,
    #[serde(default)]
    pub write_set: Vec<String>,
    #[serde(default)]
    pub lock_scope: Vec<String>,
    #[serde(default)]
    pub wait_gate: Option<Value>,
    #[serde(default)]
    pub evidence_required: Vec<String>,
    #[serde(default)]
    pub retry_policy: Option<RetryPolicy>,
    pub on_failure: OnFailure,
    #[serde(default)]
    pub extensions: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub edge_kind: Option<String>,
}

impl GraphEdge {
    pub fn kind(&self) -> &str {
        self.edge_kind.as_deref().unwrap_or("NORMAL")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputeGraph {
    pub graph_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub entry_nodes: Vec<String>,
    pub completion_nodes: Vec<String>,
}

impl ComputeGraph {
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.node_id == id)
    }
}

/// The seven graph invariants (CL-164).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Invariant {
    /// 1. No mutation without authorization and a declared write_set.
    I1MutationAuthorized,
    /// 2. No completion without criteria-linked evidence.
    I2CompletionEvidence,
    /// 3. S2 output is a candidate: never writes, always reaches a verify/policy node.
    I3S2Candidate,
    /// 4. Retries must change evidence, strategy, context or model.
    I4RetryChangesInput,
    /// 5. Every node produces a receipt/trace-backed output.
    I5NodeReceipt,
    /// 6. Projections are disposable: the graph never depends on a projection.
    I6ProjectionDisposable,
    /// 7. Routing can change models without a graph change: no model pinning.
    I7NoModelPinning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Invariant { invariant: Invariant, node_id: String, detail: String },
    Structure { node_id: Option<String>, detail: String },
}

fn inv(invariant: Invariant, n: &GraphNode, detail: &str) -> Violation {
    Violation::Invariant { invariant, node_id: n.node_id.clone(), detail: detail.to_string() }
}

fn st(node_id: Option<&str>, detail: String) -> Violation {
    Violation::Structure { node_id: node_id.map(str::to_string), detail }
}

const PROJECTION_PREFIXES: [&str; 3] = ["wih:", "dag:", "projection:"];
const MODEL_KEYS: [&str; 5] = ["model", "model_id", "provider", "provider_id", "model_pin"];

fn is_projection_ref(s: &str) -> bool {
    PROJECTION_PREFIXES.iter().any(|p| s.starts_with(p))
}

/// Validate a graph; empty result means valid.
pub fn validate(g: &ComputeGraph, reg: &PrimitiveRegistry) -> Vec<Violation> {
    let mut v = Vec::new();
    let mut ids: HashSet<&str> = HashSet::new();
    for n in &g.nodes {
        if !ids.insert(n.node_id.as_str()) {
            v.push(st(Some(&n.node_id), "duplicate node_id".into()));
        }
    }
    if g.nodes.is_empty() {
        v.push(st(None, "graph has no nodes".into()));
    }
    for e in &g.entry_nodes {
        if !ids.contains(e.as_str()) {
            v.push(st(Some(e), "entry node not in graph".into()));
        }
    }
    if g.completion_nodes.is_empty() {
        v.push(st(None, "no completion nodes".into()));
    }
    for c in &g.completion_nodes {
        if !ids.contains(c.as_str()) {
            v.push(st(Some(c), "completion node not in graph".into()));
        }
    }
    for e in &g.edges {
        for end in [&e.from, &e.to] {
            if !ids.contains(end.as_str()) {
                v.push(st(Some(end), "edge endpoint not in graph".into()));
            }
        }
    }
    for n in &g.nodes {
        if reg.resolve(&n.primitive_id).is_err() {
            v.push(st(Some(&n.node_id), format!("unknown primitive '{}'", n.primitive_id)));
        }
        if n.node_kind == "WAIT" && n.wait_gate.is_none() {
            v.push(st(Some(&n.node_id), "WAIT node requires wait_gate".into()));
        }
        if matches!(n.on_failure.strategy.as_str(), "FALLBACK" | "ROLLBACK" | "ESCALATE") {
            match &n.on_failure.target {
                Some(t) if ids.contains(t.as_str()) => {}
                _ => v.push(st(Some(&n.node_id), "on_failure target missing or unknown".into())),
            }
        }
    }
    if has_cycle(g) {
        v.push(st(None, "cycle through non-LOOP edges".into()));
    }
    invariants(g, reg, &mut v);
    v
}

fn successors<'a>(g: &'a ComputeGraph, id: &str) -> Vec<&'a str> {
    g.edges.iter().filter(|e| e.from == id).map(|e| e.to.as_str()).collect()
}

fn has_cycle(g: &ComputeGraph) -> bool {
    let mut indeg: HashMap<&str, usize> = g.nodes.iter().map(|n| (n.node_id.as_str(), 0)).collect();
    let normal: Vec<&GraphEdge> = g.edges.iter().filter(|e| e.kind() != "LOOP").collect();
    for e in &normal {
        if let Some(d) = indeg.get_mut(e.to.as_str()) {
            *d += 1;
        }
    }
    let mut q: Vec<&str> = indeg.iter().filter(|(_, d)| **d == 0).map(|(k, _)| *k).collect();
    let mut seen = 0;
    while let Some(x) = q.pop() {
        seen += 1;
        for e in normal.iter().filter(|e| e.from == x) {
            if let Some(d) = indeg.get_mut(e.to.as_str()) {
                *d -= 1;
                if *d == 0 {
                    q.push(e.to.as_str());
                }
            }
        }
    }
    seen != indeg.len()
}

/// Nodes that can reach `target` (strict ancestors) via any edge.
fn ancestors<'a>(g: &'a ComputeGraph, target: &str) -> HashSet<&'a str> {
    let mut out = HashSet::new();
    let mut stack = vec![target];
    while let Some(t) = stack.pop() {
        for e in g.edges.iter().filter(|e| e.to == t) {
            if out.insert(e.from.as_str()) {
                stack.push(e.from.as_str());
            }
        }
    }
    out
}

fn reaches_verify_or_policy(g: &ComputeGraph, from: &str) -> bool {
    let mut seen = HashSet::new();
    let mut stack = successors(g, from);
    while let Some(x) = stack.pop() {
        if !seen.insert(x) {
            continue;
        }
        if let Some(n) = g.node(x) {
            if matches!(n.node_kind.as_str(), "VERIFY" | "POLICY") {
                return true;
            }
        }
        stack.extend(successors(g, x));
    }
    false
}

fn invariants(g: &ComputeGraph, reg: &PrimitiveRegistry, v: &mut Vec<Violation>) {
    for n in &g.nodes {
        let is_mut = reg.resolve(&n.primitive_id).map(|p| p.id.starts_with("mut.")).unwrap_or(false);
        // I1
        if is_mut {
            if n.write_set.is_empty() {
                v.push(inv(Invariant::I1MutationAuthorized, n, "mutation node has empty write_set"));
            }
            if n.capability_request.is_none() {
                v.push(inv(Invariant::I1MutationAuthorized, n, "mutation node has no capability_request (authorization)"));
            }
        }
        // I2
        if g.completion_nodes.contains(&n.node_id) {
            if n.evidence_required.is_empty() {
                v.push(inv(Invariant::I2CompletionEvidence, n, "completion node has no evidence_required"));
            }
            let anc = ancestors(g, &n.node_id);
            let has_verify = n.node_kind == "VERIFY"
                || g.nodes.iter().any(|m| m.node_kind == "VERIFY" && anc.contains(m.node_id.as_str()));
            if !has_verify {
                v.push(inv(Invariant::I2CompletionEvidence, n, "no VERIFY node upstream of completion node"));
            }
        }
        // I3
        if n.cognitive_role.as_deref() == Some("S2") {
            if is_mut || !n.write_set.is_empty() {
                v.push(inv(Invariant::I3S2Candidate, n, "S2 node writes state; output must be a candidate"));
            }
            if !reaches_verify_or_policy(g, &n.node_id) {
                v.push(inv(Invariant::I3S2Candidate, n, "S2 output never reaches a VERIFY/POLICY node"));
            }
        }
        // I4
        let needs_retry = n.on_failure.strategy == "RETRY" || n.primitive_id == "ctl.retry";
        match &n.retry_policy {
            None if needs_retry => v.push(inv(Invariant::I4RetryChangesInput, n, "retry without retry_policy")),
            Some(rp) if rp.must_change.is_empty() => {
                v.push(inv(Invariant::I4RetryChangesInput, n, "retry_policy.must_change is empty"))
            }
            Some(rp) if rp.max_attempts == 0 && needs_retry => {
                v.push(inv(Invariant::I4RetryChangesInput, n, "retry strategy with max_attempts 0"))
            }
            _ => {}
        }
        // I5
        let skip = n
            .extensions
            .as_ref()
            .map(|e| e.get("skip_receipt").and_then(Value::as_bool).unwrap_or(false))
            .unwrap_or(false);
        if skip || (n.node_kind != "CONTROL" && n.outputs.is_empty()) {
            v.push(inv(Invariant::I5NodeReceipt, n, "node declares no receipt-backed output"));
        }
        // I6
        if n.inputs.iter().chain(&n.read_set).chain(&n.write_set).any(|s| is_projection_ref(s)) {
            v.push(inv(Invariant::I6ProjectionDisposable, n, "node references a projection (wih/dag) as truth"));
        }
        // I7
        if let Some(ext) = &n.extensions {
            if MODEL_KEYS.iter().any(|k| ext.contains_key(*k)) {
                v.push(inv(Invariant::I7NoModelPinning, n, "node pins a model/provider in extensions"));
            }
        }
        if let Some(Value::Object(c)) = &n.capability_request {
            if MODEL_KEYS.iter().any(|k| c.contains_key(*k)) {
                v.push(inv(Invariant::I7NoModelPinning, n, "capability_request pins a model/provider"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reg() -> &'static PrimitiveRegistry {
        PrimitiveRegistry::global()
    }

    fn node(id: &str, prim: &str, kind: &str) -> Value {
        json!({"node_id": id, "primitive_id": prim, "node_kind": kind,
               "inputs": [], "outputs": [format!("out:{id}")], "read_set": [],
               "write_set": [], "lock_scope": [], "on_failure": {"strategy": "FAIL"}})
    }

    /// Valid 4-node graph: observe -> patch(mut) -> verify -> finish.
    fn good() -> Value {
        let mut patch = node("patch", "mut.apply_patch", "COMPUTE");
        patch["write_set"] = json!(["fs:src/app.py"]);
        patch["capability_request"] = json!({"capability": "code.edit"});
        let mut verify = node("verify", "ver.unit_test", "VERIFY");
        verify["evidence_required"] = json!(["test_report"]);
        let mut fin = node("fin", "ctl.finish", "CONTROL");
        fin["evidence_required"] = json!(["test_report"]);
        json!({"graph_id":"g1","entry_nodes":["obs"],"completion_nodes":["fin"],
          "nodes":[node("obs","obs.observe_environment","COMPUTE"),patch,verify,fin],
          "edges":[{"from":"obs","to":"patch"},{"from":"patch","to":"verify"},{"from":"verify","to":"fin"}]})
    }

    fn check(v: Value) -> Vec<Violation> {
        let g: ComputeGraph = serde_json::from_value(v).unwrap();
        validate(&g, reg())
    }

    fn has(vs: &[Violation], i: Invariant) -> bool {
        vs.iter().any(|x| matches!(x, Violation::Invariant { invariant, .. } if *invariant == i))
    }

    #[test]
    fn good_graph_is_valid() {
        assert_eq!(check(good()), vec![]);
    }

    #[test]
    fn i1_mutation_needs_write_set_and_authorization() {
        let mut g = good();
        g["nodes"][1]["write_set"] = json!([]);
        assert!(has(&check(g), Invariant::I1MutationAuthorized));
        let mut g = good();
        g["nodes"][1]["capability_request"] = Value::Null;
        assert!(has(&check(g), Invariant::I1MutationAuthorized));
    }

    #[test]
    fn i2_completion_needs_evidence_and_verify() {
        let mut g = good();
        g["nodes"][3]["evidence_required"] = json!([]);
        assert!(has(&check(g), Invariant::I2CompletionEvidence));
        let mut g = good();
        g["nodes"][2]["node_kind"] = json!("COMPUTE");
        assert!(has(&check(g), Invariant::I2CompletionEvidence));
    }

    #[test]
    fn i3_s2_is_candidate() {
        let mut g = good();
        g["nodes"][0]["cognitive_role"] = json!("S2");
        g["nodes"][0]["write_set"] = json!(["fs:x"]);
        assert!(has(&check(g), Invariant::I3S2Candidate));
        // S2 that never reaches verify/policy
        let mut g = good();
        let mut s2 = node("gen", "obs.observe_environment", "COMPUTE");
        s2["cognitive_role"] = json!("S2");
        g["nodes"].as_array_mut().unwrap().push(s2);
        g["edges"].as_array_mut().unwrap().push(json!({"from":"obs","to":"gen"}));
        assert!(has(&check(g), Invariant::I3S2Candidate));
    }

    #[test]
    fn i3_s2_reaching_verify_is_ok() {
        let mut g = good();
        g["nodes"][0]["cognitive_role"] = json!("S2");
        assert_eq!(check(g), vec![]);
    }

    #[test]
    fn i4_retry_must_change_something() {
        let mut g = good();
        g["nodes"][0]["on_failure"] = json!({"strategy":"RETRY"});
        assert!(has(&check(g), Invariant::I4RetryChangesInput));
        let mut g = good();
        g["nodes"][0]["retry_policy"] = json!({"max_attempts":2,"must_change":[]});
        assert!(has(&check(g), Invariant::I4RetryChangesInput));
        let mut g = good();
        g["nodes"][0]["on_failure"] = json!({"strategy":"RETRY"});
        g["nodes"][0]["retry_policy"] = json!({"max_attempts":2,"must_change":["EVIDENCE"]});
        assert_eq!(check(g), vec![]);
    }

    #[test]
    fn i5_every_node_has_receipt_output() {
        let mut g = good();
        g["nodes"][0]["outputs"] = json!([]);
        assert!(has(&check(g), Invariant::I5NodeReceipt));
        let mut g = good();
        g["nodes"][0]["extensions"] = json!({"skip_receipt": true});
        assert!(has(&check(g), Invariant::I5NodeReceipt));
    }

    #[test]
    fn i6_projection_not_truth() {
        let mut g = good();
        g["nodes"][0]["inputs"] = json!(["wih:task-1"]);
        assert!(has(&check(g), Invariant::I6ProjectionDisposable));
    }

    #[test]
    fn i7_no_model_pinning() {
        let mut g = good();
        g["nodes"][0]["extensions"] = json!({"model": "some-model"});
        assert!(has(&check(g), Invariant::I7NoModelPinning));
        let mut g = good();
        g["nodes"][1]["capability_request"] = json!({"capability":"x","provider":"p"});
        assert!(has(&check(g), Invariant::I7NoModelPinning));
    }

    #[test]
    fn structural_rules() {
        let mut g = good();
        g["edges"].as_array_mut().unwrap().push(json!({"from":"fin","to":"obs"}));
        assert!(check(g).iter().any(|x| matches!(x, Violation::Structure { detail, .. } if detail.contains("cycle"))));
        let mut g = good();
        g["edges"].as_array_mut().unwrap().push(json!({"from":"fin","to":"obs","edge_kind":"LOOP"}));
        assert_eq!(check(g), vec![]);
        let mut g = good();
        g["nodes"][0]["primitive_id"] = json!("obs.nope");
        assert!(check(g).iter().any(|x| matches!(x, Violation::Structure { detail, .. } if detail.contains("unknown primitive"))));
        let mut g = good();
        g["edges"].as_array_mut().unwrap().push(json!({"from":"obs","to":"ghost"}));
        assert!(!check(g).is_empty());
    }
}
