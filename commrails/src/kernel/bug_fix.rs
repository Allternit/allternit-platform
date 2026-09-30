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
