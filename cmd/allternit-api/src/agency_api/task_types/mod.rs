//! Kernel task types beyond BUG_FIX (W5 / WP-X1).
//!
//! Each task type is three versioned data files plus this registry:
//! * a compute graph (`graphs/<graph_id>.json`, ComputeGraphIRV1) that the
//!   commrails validator accepts (the seven invariants, registry membership);
//! * a completion contract (`contracts.v1.json`, CompletionPolicyV1 shape):
//!   what "done" means. The graph's `gate` node (`ver.check_acceptance_evidence`,
//!   verifier-owned, S0) requires exactly the contract's criteria, and every
//!   criterion is produced by exactly one VERIFY / POLICY / WAIT / CONTROL node
//!   (`x-evidence`), never by a generative node;
//! * an eval set (`evals/<task_type>.v1.json`) run by [`runner::run_scripted`]
//!   with the scripted executor (no model) in this module's tests.
//!
//! No model or vendor identity anywhere: generative nodes ask for logical
//! `cap.<type>.*` capabilities and the router picks the backend.
//!
//! Registration: [`template`] is reached from `compiler::TemplateRegistry::get`
//! and [`catalog_contract`] / [`criteria`] from `catalog` (small hooks marked
//! "WP-X1 hook"). The executor still drives BUG_FIX only; driving these graphs
//! for real lands after the executor-core work (WP-P1).

pub mod runner;
#[cfg(test)]
mod tests;

use super::compiler::{RunTemplate, TemplateGraph};
use allternit_commrails::judge::completion::CompletionPolicy;
use allternit_commrails::kernel::graph::{self, ComputeGraph};
use allternit_commrails::kernel::registry::PrimitiveRegistry;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;

/// One registered task type.
#[derive(Debug, Clone, Copy)]
pub struct TaskType {
    pub id: &'static str,
    pub graph_id: &'static str,
    graph_json: &'static str,
    /// Required scheme of the declared writable resources: `None` = the graph
    /// never writes; `Some("")` = any `scheme:value` ref.
    pub write_scheme: Option<&'static str>,
}

impl TaskType {
    pub fn completion_policy_id(&self) -> String { format!("completion.{}", self.id.to_ascii_lowercase()) }
}

pub const TEMPLATE_VERSION: u32 = 1;
pub const TEMPLATE_SOURCE: &str = "kernel";

pub const TASK_TYPES: &[TaskType] = &[
    TaskType { id: "GENERAL_TASK", graph_id: "general.task.v1", graph_json: include_str!("graphs/general.task.v1.json"), write_scheme: None },
    TaskType { id: "THREAD_WORK", graph_id: "bots.thread_work.v1", graph_json: include_str!("graphs/bots.thread_work.v1.json"), write_scheme: Some("thread") },
    TaskType { id: "COMPUTER_USE", graph_id: "computer.use.v1", graph_json: include_str!("graphs/computer.use.v1.json"), write_scheme: Some("computer") },
    TaskType { id: "RESEARCH_DOC", graph_id: "research.doc.v1", graph_json: include_str!("graphs/research.doc.v1.json"), write_scheme: Some("fs") },
    TaskType { id: "TEMPLATE", graph_id: "template.run.v1", graph_json: include_str!("graphs/template.run.v1.json"), write_scheme: Some("") },
    TaskType { id: "CAMPAIGN", graph_id: "campaign.cycle.v1", graph_json: include_str!("graphs/campaign.cycle.v1.json"), write_scheme: Some("campaign") },
];

const CONTRACTS: &str = include_str!("contracts.v1.json");

/// Criteria the new contracts introduce (`requirements_satisfied` is shared
/// with BUG_FIX and already in the catalog).
const CRITERIA: &[(&str, &str)] = &[
    ("response_produced", "A response exists for the request."),
    ("user_acceptance", "The requesting user accepted the result."),
    ("post_authorized", "Policy authorized posting to the thread."),
    ("message_delivered", "The reply was delivered to the thread."),
    ("action_authorized", "Policy authorized the computer action."),
    ("screenshot_evidence", "A screenshot taken after the action shows its effect."),
    ("citations_resolve", "Every claim cites a source that was actually read."),
    ("document_exists", "The document was written where it was declared."),
    ("template_steps_complete", "Every template step completed with no unresolved failure."),
    ("checks_pass", "The task's declared checks pass."),
    ("cycle_triggered", "The cycle was started by an explicit trigger, not a timer."),
    ("step_authorized", "Policy authorized the campaign step."),
    ("checkpoint_persisted", "Campaign state was checkpointed after the step."),
];

pub fn get(id: &str) -> Option<&'static TaskType> {
    TASK_TYPES.iter().find(|t| t.id == id)
}

pub fn ids() -> Vec<&'static str> {
    TASK_TYPES.iter().map(|t| t.id).collect()
}

/// The unbound graph as shipped.
pub fn graph(t: &TaskType) -> Result<ComputeGraph> {
    Ok(ComputeGraph::from_json(t.graph_json)?)
}

/// Instantiate with the task's declared writable resources. Fails closed when
/// a writing graph has none, or a resource has the wrong scheme. Policy still
/// has to authorize the actual effect at the graph's POLICY node.
pub fn instantiate(t: &TaskType, task_id: &str, writable: &[String]) -> Result<ComputeGraph> {
    if task_id.is_empty() {
        bail!("{} requires a task id", t.id);
    }
    let mut g = graph(t)?;
    g.task_id = Some(task_id.to_owned());
    match t.write_scheme {
        None if !writable.is_empty() => bail!("{} never writes; declare no writable resources", t.id),
        None => {}
        Some(scheme) => {
            if writable.is_empty() {
                bail!("{} requires declared writable resources", t.id);
            }
            for r in writable {
                let ok = match r.split_once(':') {
                    Some((s, v)) => !v.is_empty() && !s.is_empty() && (scheme.is_empty() || s == scheme),
                    None => false,
                };
                if !ok {
                    bail!("{} writable resources must be `{}:<ref>` refs (got `{r}`)", t.id, if scheme.is_empty() { "<scheme>" } else { scheme });
                }
            }
            for n in g.nodes.iter_mut().filter(|n| binds_write_set(n)) {
                n.write_set = writable.to_vec();
                n.lock_scope = writable.to_vec();
            }
        }
    }
    let errors = graph::validate(&g, PrimitiveRegistry::global());
    if !errors.is_empty() {
        bail!("invalid {} graph: {errors:?}", t.id);
    }
    Ok(g)
}

fn binds_write_set(n: &graph::GraphNode) -> bool {
    n.extensions.as_ref().and_then(|e| e.get("x-bind_write_set")).and_then(Value::as_bool).unwrap_or(false)
}

/// The criterion a node's committed output is evidence for, if any.
pub fn evidence_of(n: &graph::GraphNode) -> Option<&str> {
    n.extensions.as_ref()?.get("x-evidence")?.as_str()
}

fn contracts_json() -> Vec<Value> {
    serde_json::from_str(CONTRACTS).expect("embedded task-type contracts are valid JSON")
}

/// The task type's completion contract as the kernel's `CompletionPolicy`.
pub fn completion_policy(t: &TaskType) -> Option<CompletionPolicy> {
    let id = t.completion_policy_id();
    contracts_json().into_iter().find(|c| c["policy_id"] == id.as_str()).and_then(|c| serde_json::from_value(c).ok())
}

/// WP-X1 hook for `catalog::completion_contract`: the contract in the
/// catalog's shape (`{id, version, task_type, allow_partial, require}`).
pub fn catalog_contract(policy_id: &str) -> Option<Value> {
    let c = contracts_json().into_iter().find(|c| c["policy_id"] == policy_id)?;
    let require: Vec<Value> = c["require"].as_array()?.iter()
        .map(|r| json!({ "id": r["criterion_id"], "version": r["criterion_version"] }))
        .collect();
    Some(json!({ "id": c["policy_id"], "version": c["policy_version"], "task_type": c["task_type"], "allow_partial": c["allow_partial"], "require": require }))
}

/// WP-X1 hook for `catalog::criteria`: the new criteria ids + descriptions.
pub fn criteria() -> &'static [(&'static str, &'static str)] {
    CRITERIA
}

/// `RunTemplate` over a registered task type (same output shape as BUG_FIX's
/// `agency_graph`).
pub struct KernelTaskTemplate(pub &'static TaskType);

impl RunTemplate for KernelTaskTemplate {
    fn id(&self) -> &'static str { self.0.id }
    fn version(&self) -> u32 { TEMPLATE_VERSION }
    fn source(&self) -> &'static str { TEMPLATE_SOURCE }
    fn completion_policy(&self) -> &'static str {
        // The trait wants a `'static str`; tests check this equals `completion_policy_id()`.
        match self.0.id {
            "GENERAL_TASK" => "completion.general_task",
            "THREAD_WORK" => "completion.thread_work",
            "COMPUTER_USE" => "completion.computer_use",
            "RESEARCH_DOC" => "completion.research_doc",
            "TEMPLATE" => "completion.template",
            "CAMPAIGN" => "completion.campaign",
            _ => "completion.unknown",
        }
    }
    fn instantiate(&self, goal: &str, params: &Value) -> Result<TemplateGraph, String> {
        let t = self.0;
        let writable: Vec<String> = params["writable_resources"].as_array()
            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default();
        let task_id = params["task_id"].as_str().map(str::to_owned).unwrap_or_else(|| {
            format!("task.{}.{}", t.id.to_ascii_lowercase(), &allternit_commrails::receipts::jcs::sha256_tagged(goal.as_bytes())[7..23])
        });
        let g = instantiate(t, &task_id, &writable).map_err(|e| e.to_string())?;
        to_template_graph(&g, &writable, t.graph_id, &task_id).map_err(|e| e.to_string())
    }
}

fn to_template_graph(g: &ComputeGraph, writable: &[String], graph_id: &str, task_id: &str) -> Result<TemplateGraph> {
    let nodes = g.nodes.iter().map(|n| {
        let mut v = serde_json::to_value(n)?;
        v["id"] = n.node_id.clone().into();
        v["role"] = n.primitive_id.clone().into();
        v["writes"] = (!n.write_set.is_empty()).into();
        Ok(v)
    }).collect::<Result<Vec<_>>>()?;
    let edges = g.edges.iter().map(serde_json::to_value).collect::<serde_json::Result<Vec<_>>>()?;
    Ok(TemplateGraph { nodes, edges, wih_policy: json!({
        "requires_lease_for_write": true, "write_set": writable, "graph_id": graph_id, "task_id": task_id,
    }) })
}

/// WP-X1 hook for `compiler::TemplateRegistry::get`.
pub fn template(id: &str) -> Option<Arc<dyn RunTemplate>> {
    get(id).map(|t| Arc::new(KernelTaskTemplate(t)) as Arc<dyn RunTemplate>)
}

/// Look a task type up or say which ones exist.
pub fn require(id: &str) -> Result<&'static TaskType> {
    get(id).ok_or_else(|| anyhow!("unknown task type `{id}` (known: {})", ids().join(", ")))
}
