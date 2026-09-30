//! BUG_FIX v1: canonical N00–N23 plus explicit cognitive fallbacks, S3
//! escalation, rollback, evidence requests and response acceptance.
//! S1 is advisory until calibration passes Q22. N21 is a deterministic,
//! verifier-owned completion gate. S2 emits candidates, including the response.

use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::Value;
use super::{graph::{self, ComputeGraph}, registry::PrimitiveRegistry};

pub const TEMPLATE_ID: &str = "BUG_FIX";
pub const GRAPH_ID: &str = "coding.bug_fix.v1";

/// Instantiate with the task's declared writable resources, never inferred
/// model authority. Policy still has to authorize the actual mutation at N13.
pub fn instantiate(task_id: &str, writable_resources: &[String]) -> Result<ComputeGraph> {
    if task_id.is_empty() || writable_resources.is_empty() {
        bail!("BUG_FIX requires a task id and declared writable resources");
    }
    if writable_resources.iter().any(|s| !s.starts_with("fs:") || s.len() <= 3) {
        bail!("BUG_FIX writable resources must be filesystem resource refs");
    }
    let mut g = graph()?;
    g.task_id = Some(task_id.to_owned());
    let mutation = g.nodes.iter_mut().find(|n| n.node_id == "N14").unwrap();
    mutation.write_set = writable_resources.to_vec();
    mutation.lock_scope = writable_resources.to_vec();
    let errors = graph::validate(&g, PrimitiveRegistry::global());
    if !errors.is_empty() { bail!("invalid BUG_FIX graph: {errors:?}"); }
    Ok(g)
}

pub const TEMPLATE_VERSION: u32 = 1;
/// `source` the Agency API reports for this template (the WP11 stub says `stub`).
pub const TEMPLATE_SOURCE: &str = "kernel";
pub const COMPLETION_POLICY: &str = "completion.bug_fix";

/// Shape of the Agency API's `TemplateGraph` (WP11 `compiler::RunTemplate`).
#[derive(Debug, Clone)]
pub struct AgencyTemplateGraph {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub wih_policy: Value,
}

/// Public entry point for the Agency API (WP11 `RunTemplate::instantiate`).
/// `params.writable_resources` (fs: refs) wins; otherwise `params.workspace`
/// becomes `fs:<workspace>`. Fails closed when neither is declared: write
/// authority is never inferred from the goal text.
/// Each node is the full GraphNode plus the stub's `id`/`role`/`writes` keys
/// (`role` = primitive id) so the WP11 graph view keeps working unchanged.
pub fn agency_graph(goal: &str, params: &Value) -> Result<AgencyTemplateGraph> {
    let writable: Vec<String> = match params["writable_resources"].as_array() {
        Some(a) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
        None => params["workspace"].as_str().filter(|w| !w.is_empty()).map(|w| vec![format!("fs:{w}")]).unwrap_or_default(),
    };
    let task_id = match params["task_id"].as_str() {
        Some(t) => t.to_owned(),
        None => format!("task.bug_fix.{}", &crate::receipts::jcs::sha256_tagged(goal.as_bytes())[7..23]),
    };
    let g = instantiate(&task_id, &writable)?;
    let nodes = g.nodes.iter().map(|n| {
        let mut v = serde_json::to_value(n)?;
        v["id"] = n.node_id.clone().into();
        v["role"] = n.primitive_id.clone().into();
        v["writes"] = (!n.write_set.is_empty()).into();
        Ok(v)
    }).collect::<Result<Vec<_>>>()?;
    let edges = g.edges.iter().map(serde_json::to_value).collect::<serde_json::Result<Vec<_>>>()?;
    Ok(AgencyTemplateGraph { nodes, edges, wih_policy: serde_json::json!({
        "requires_lease_for_write": true, "write_set": writable, "graph_id": GRAPH_ID, "task_id": task_id,
    }) })
}

pub fn graph() -> Result<ComputeGraph> {
    Ok(serde_json::from_str(include_str!("templates/bug_fix.v1.json"))?)
}

#[derive(Debug, Deserialize)]
pub struct ErrorOntology {
    pub bank_id: String,
    pub version: String,
    pub primitive_id: String,
    pub classes: Vec<String>,
    pub unknown: String,
}

pub fn error_ontology() -> ErrorOntology {
    serde_json::from_str(include_str!("templates/error_ontology.v0.1.json")).expect("embedded error bank")
}

/// Deterministic diagnostics may supply a known class; uncertain diagnostics
/// stay UNKNOWN for the classification capability to inspect, never guessed.
pub fn error_class(code: &str) -> String {
    let bank = error_ontology();
    if bank.classes.iter().any(|c| c == code) { code.to_owned() } else { bank.unknown }
}

/// S0 test/parse step hook: the deterministic diagnostic `code` is ground truth for an
/// S1 CLASSIFY_ERROR decision made on the same failure. Reports it (known class only,
/// fire-and-forget) when the decision result carries a decision id.
pub fn reconcile_s0_classification(
    reporter: &super::s1_outcome::OutcomeReporter,
    s1_result: Option<&super::router::DecisionResultView>,
    code: &str,
) -> String {
    if let Some(id) = s1_result.and_then(|r| r.decision_id()) {
        super::s1_outcome::report_classify_outcome(reporter, &id, code, "bug_fix.s0");
    }
    error_class(code)
}

#[derive(Debug, Deserialize)]
pub struct VerificationStep {
    pub step_id: String,
    pub primitive_id: String,
    pub deterministic: bool,
}

pub fn verification_ladder() -> Vec<VerificationStep> {
    serde_json::from_str(include_str!("templates/verification_ladder.v1.json")).expect("embedded verification ladder")
}

/// An inconclusive or error result cannot advance the ladder. An inapplicable
/// step needs an explicit reason; it cannot satisfy a completion criterion.
pub fn step_passes(result: &str, deterministic_required: bool, receipt: &Value) -> bool {
    result == "PASS" && (!deterministic_required || receipt["deterministic"] == true)
        && receipt["evidence_refs"].as_array().is_some_and(|e| !e.is_empty())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod e2e_tests;
