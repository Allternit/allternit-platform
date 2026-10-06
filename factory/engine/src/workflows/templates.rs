//! Workflow templates.
//!
//! Templates are reusable plans. They instantiate into:
//!
//! - **WIH DAG nodes** (the default task system in this tree):
//!   [`plan_from_template`] mints `DagNodeCreated` + `DagEdgeAdded(blocked_by)`
//!   (+ `DagNodeWaitGateAdded`) mutations and applies them through the normal
//!   Gate 0 path (`plan_new` + `plan_refine`), so every node carries prompt
//!   delta provenance.
//! - **Tickets** (portable CLI, out-of-scope tooling for foreign repos):
//!   [`TemplateStore::instantiate`], unchanged.
//!
//! Two on-disk formats live in [`TEMPLATE_DIR`]:
//!
//! - `<id>.json` — a serialized [`Template`] (what `allternit-factory internal core template new`
//!   writes).
//! - `<id>.md` — markdown with YAML frontmatter (`name`, `description`) and
//!   exactly one fenced ```` ```yaml template-spec ```` block holding
//!   `params` and `steps` (see `spec/DAG_AS_DEFAULT_TASK_SYSTEM.md`).
//!
//! Step text may use `{{ params.<name> }}` (substituted at instantiation;
//! missing required params are rejected) and `{{ <step_id>.output }}` /
//! `{{ <step_id>.output_path }}` (rewritten to the minted node ids and
//! resolved at pickup).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};
use crate::dependencies::{DependencyEdge, DependencyGraph, DependencyKind};
use crate::gate::gate::{DagMutation, PromptOrigin};
use crate::gate::Gate;
use crate::rails_id::{HierarchicalId, TicketId};
use crate::tickets::{Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore};
use crate::wait_gates::WaitGateKind;
use crate::work::placeholders;
use crate::work::types::{validate_executor, validate_template_executor, DagEdge};

/// Kernel templates keep ComputeGraphIR authoritative (the WIH DAG is a projection).
pub use crate::kernel::bug_fix as bug_fix;

/// Default directory for templates, relative to workspace root.
pub const TEMPLATE_DIR: &str = ".allternit/rails/templates";

/// A step inside a template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateStep {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Ticket kind (ticket instantiation only).
    #[serde(default)]
    pub kind: TicketKind,
    /// Ticket priority (ticket instantiation only).
    #[serde(default)]
    pub priority: TicketPriority,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// `bot:<slug>` | `ao:<harness>` | `role:<role>` (WIH DAG only; `drive`
    /// spawns `ao:` and mails `bot:`). `role:` is resolved from the team's
    /// role map at instantiation, so nodes only ever carry `bot:` / `ao:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// Wait-gate attached to the node (WIH DAG only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_gate: Option<TemplateWaitGate>,
    /// `safe`: the step is idempotent, so `drive` may restart it after an
    /// interrupted attempt (becomes the node label `retry:safe`). Anything
    /// else is rejected; omit it for steps with non-idempotent effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
    /// Step to route back to when this step closes failed. Must be a
    /// transitive `blocked_by` predecessor. Becomes the node labels
    /// `on_fail:<target node id>` and `max_rounds:<n>`; `drive` reopens the
    /// target and every node between it and this one, at most `max_rounds`
    /// times, then stops with a degraded closure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_fail: Option<String>,
}

/// Node label that marks a node safe for `drive` to restart after an
/// interrupted attempt.
pub const RETRY_SAFE_LABEL: &str = "retry:safe";

/// Node label prefix naming the node a failed node routes back to:
/// `on_fail:<target node id>`.
pub const ON_FAIL_LABEL_PREFIX: &str = "on_fail:";

/// Node label prefix carrying the route-back limit: `max_rounds:<n>`.
pub const MAX_ROUNDS_LABEL_PREFIX: &str = "max_rounds:";

/// `max_rounds` when a template has an `on_fail` step but declares no
/// `max_rounds` (determinism contract rule 10: every loop has a limit).
pub const DEFAULT_MAX_ROUNDS: u32 = 3;

/// Root state dimension holding the closure the flow ended with
/// (`degraded` is written by `drive` when route-back rounds run out).
pub const CLOSURE_STATE: &str = "closure";
/// Root state dimensions holding the template's closure texts.
pub const CLOSURE_SUCCESS_STATE: &str = "closure_success";
pub const CLOSURE_DEGRADED_STATE: &str = "closure_degraded";
pub const CLOSURE_FAILED_STATE: &str = "closure_failed";

/// Wait-gate param carrying what the person must look at before approving.
pub const EVIDENCE_PARAM: &str = "evidence";

/// What the campaign/root records on each closure, one line each.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateClosure {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// A wait-gate declared on a template step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateWaitGate {
    pub kind: WaitGateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What the person must look at (e.g. `PROOF.md and proof/ files`).
    /// Carried into the gate's `params.evidence` and its description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// `until` (timer, RFC 3339), `repo`, `run_id`, `pr`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, serde_json::Value>,
}

/// A declared template parameter. No default = required.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateParam {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// A reusable workflow template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<TemplateParam>,
    pub steps: Vec<TemplateStep>,
    /// Route-back limit for `on_fail` steps (positive). Defaults to
    /// [`DEFAULT_MAX_ROUNDS`] when any step has `on_fail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    /// Closure texts, recorded on the plan root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<TemplateClosure>,
    #[serde(default = "Utc::now")]
    pub created_at: chrono::DateTime<Utc>,
    /// Compiled into the engine (no workspace file of this id). Never read
    /// from disk.
    #[serde(default, skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
}

/// Role -> executor (`bot:<slug>` / `ao:<harness>`), built by the caller
/// from the team's `team.yaml`.
pub type RoleMap = BTreeMap<String, String>;

/// Result of instantiating a template into tickets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstantiationResult {
    pub template_id: String,
    pub root_id: TicketId,
    pub tickets: Vec<Ticket>,
}

/// Mutations a template expands to for one WIH DAG instantiation.
#[derive(Clone, Debug, Serialize)]
pub struct DagTemplateExpansion {
    /// step id -> minted node id.
    pub nodes: BTreeMap<String, String>,
    /// CreateNode (children of the root) + AddBlockedBy + AddWaitGate.
    pub mutations: Vec<DagMutation>,
    /// Effective params (provided + defaults).
    pub params: BTreeMap<String, String>,
}

/// Result of [`plan_from_template`].
#[derive(Clone, Debug, Serialize)]
pub struct TemplatePlanResult {
    pub template_id: String,
    pub prompt_id: String,
    pub dag_id: String,
    pub root_node_id: String,
    pub delta_id: String,
    pub nodes: BTreeMap<String, String>,
    pub params: BTreeMap<String, String>,
}

impl Template {
    /// Effective params: declared defaults overlaid with `provided`. Rejects
    /// unknown provided names (when params are declared) and any required
    /// param — declared without default, or referenced as
    /// `{{ params.<name> }}` without being declared — that is missing.
    pub fn resolve_params(&self, provided: &HashMap<String, String>) -> Result<BTreeMap<String, String>> {
        let declared: HashSet<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
        let referenced = self.referenced_params();
        let mut unknown: Vec<&String> = provided
            .keys()
            .filter(|k| !declared.contains(k.as_str()) && !referenced.contains(k.as_str()))
            .collect();
        unknown.sort();
        if !unknown.is_empty() {
            bail!(
                "template {} has no param(s): {}",
                self.id,
                unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            );
        }
        let mut out = BTreeMap::new();
        for p in &self.params {
            if let Some(d) = &p.default {
                out.insert(p.name.clone(), d.clone());
            }
        }
        for (k, v) in provided {
            out.insert(k.clone(), v.clone());
        }
        let mut required: Vec<String> = self
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .map(|p| p.name.clone())
            .chain(referenced.iter().map(|s| s.to_string()))
            .collect();
        required.sort();
        required.dedup();
        let missing: Vec<String> = required.into_iter().filter(|k| !out.contains_key(k)).collect();
        if !missing.is_empty() {
            bail!(
                "template {} missing required param(s): {} (pass --param <name>=<value>)",
                self.id,
                missing.join(", ")
            );
        }
        Ok(out)
    }

    fn referenced_params(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for step in &self.steps {
            out.extend(placeholders::param_refs(&step.title));
            out.extend(placeholders::param_refs(&step.description));
            if let Some(gate) = &step.wait_gate {
                if let Some(d) = &gate.description {
                    out.extend(placeholders::param_refs(d));
                }
                if let Some(e) = &gate.evidence {
                    out.extend(placeholders::param_refs(e));
                }
                for v in gate.params.values() {
                    if let Some(s) = v.as_str() {
                        out.extend(placeholders::param_refs(s));
                    }
                }
            }
        }
        if let Some(c) = &self.closure {
            for text in [&c.success, &c.degraded, &c.failed].into_iter().flatten() {
                out.extend(placeholders::param_refs(text));
            }
        }
        out
    }

    /// Route-back limit: the declared `max_rounds`, else
    /// [`DEFAULT_MAX_ROUNDS`] when any step has `on_fail`, else none.
    pub fn effective_max_rounds(&self) -> Option<u32> {
        self.max_rounds.or_else(|| {
            self.steps
                .iter()
                .any(|s| s.on_fail.is_some())
                .then_some(DEFAULT_MAX_ROUNDS)
        })
    }

    /// The template in API.md's `Template` shape (camelCase:
    /// `blockedBy`, `onFail`, `waitGate {kind, evidence?}`, `maxRounds`,
    /// `closure`). The on-disk format stays snake_case.
    pub fn to_contract_json(&self) -> serde_json::Value {
        use serde_json::json;
        let params: Vec<serde_json::Value> = self
            .params
            .iter()
            .map(|p| {
                let mut o = json!({ "name": p.name });
                if let Some(d) = &p.description {
                    o["description"] = json!(d);
                }
                if let Some(d) = &p.default {
                    o["default"] = json!(d);
                }
                o
            })
            .collect();
        let steps: Vec<serde_json::Value> = self
            .steps
            .iter()
            .map(|s| {
                let wait_gate = s.wait_gate.as_ref().map(|g| {
                    let mut o = json!({ "kind": g.kind });
                    if let Some(e) = &g.evidence {
                        o["evidence"] = json!(e);
                    }
                    o
                });
                json!({
                    "id": s.id,
                    "title": s.title,
                    "executor": s.executor,
                    "blockedBy": s.blocked_by,
                    "onFail": s.on_fail,
                    "waitGate": wait_gate,
                    "retry": s.retry,
                })
            })
            .collect();
        json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "params": params,
            "steps": steps,
            "maxRounds": self.effective_max_rounds(),
            "closure": self.closure,
        })
    }

    /// Structural validation that needs no params or team: step ids, edges
    /// (unknown / cycles), executors (`role:` allowed), `retry`, `on_fail`
    /// (existing transitive predecessor), `max_rounds` (positive), and
    /// output placeholders (predecessors only).
    pub fn validate(&self) -> Result<()> {
        if self.steps.is_empty() {
            bail!("template {} has no steps", self.id);
        }

        let mut seen = HashSet::new();
        for step in &self.steps {
            if step.id.is_empty()
                || !step
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                bail!("template step id {:?} must be [A-Za-z0-9_-]+", step.id);
            }
            if !seen.insert(step.id.as_str()) {
                bail!("template step id {} is duplicated", step.id);
            }
        }
        let mut edges: Vec<DagEdge> = Vec::new();
        for step in &self.steps {
            for blocker in &step.blocked_by {
                if !seen.contains(blocker.as_str()) {
                    bail!("step {} is blocked_by unknown step {}", step.id, blocker);
                }
                if crate::work::graph::would_create_cycle(&edges, blocker, &step.id) {
                    bail!("template blocked_by edges form a cycle at {} -> {}", blocker, step.id);
                }
                edges.push(DagEdge {
                    from_node_id: blocker.clone(),
                    to_node_id: step.id.clone(),
                    edge_type: "blocked_by".to_string(),
                });
            }
        }
        if self.max_rounds == Some(0) {
            bail!("template {}: max_rounds must be a positive integer", self.id);
        }
        for step in &self.steps {
            if let Some(executor) = &step.executor {
                validate_template_executor(executor)
                    .map_err(|e| anyhow::anyhow!("step {}: {}", step.id, e))?;
            }
            if let Some(retry) = &step.retry {
                if retry != "safe" {
                    bail!("step {}: retry must be \"safe\" (or omitted), got {:?}", step.id, retry);
                }
            }
            let preds = transitive_blockers(&self.steps, &step.id);
            if let Some(target) = &step.on_fail {
                if !seen.contains(target.as_str()) {
                    bail!("step {}: on_fail names unknown step {}", step.id, target);
                }
                if !preds.contains(target) {
                    bail!(
                        "step {}: on_fail {} must be a blocked_by predecessor of {} (the step to route back to)",
                        step.id,
                        target,
                        step.id
                    );
                }
            }
            for r in placeholders::node_refs(&step.description) {
                if !seen.contains(r.node_id.as_str()) {
                    bail!("step {}: {} references unknown step {}", step.id, r.raw, r.node_id);
                }
                if !preds.contains(&r.node_id) {
                    bail!(
                        "step {}: {} references {} which is not a blocked_by predecessor",
                        step.id,
                        r.raw,
                        r.node_id
                    );
                }
            }
        }
        Ok(())
    }

    /// Validate the template and expand it into DAG mutations whose nodes are
    /// children of `root_node_id`. Node ids are `<step_id>-<suffix>` with one
    /// random suffix per instantiation (node ids must be unique across dags).
    /// Templates with `role:` executors need [`Template::expand_dag_with_roles`].
    pub fn expand_dag(
        &self,
        root_node_id: &str,
        provided: &HashMap<String, String>,
    ) -> Result<DagTemplateExpansion> {
        self.expand_dag_with_roles(root_node_id, provided, None)
    }

    /// The executor a step's node gets: `role:<r>` replaced through `roles`
    /// (an unmapped role is an error naming it); `bot:` / `ao:` unchanged.
    pub fn resolve_step_executor(
        &self,
        step: &TemplateStep,
        roles: Option<&RoleMap>,
    ) -> Result<Option<String>> {
        let Some(executor) = &step.executor else {
            return Ok(None);
        };
        let Some(role) = executor.strip_prefix("role:") else {
            return Ok(Some(executor.clone()));
        };
        let mapped = roles.and_then(|r| r.get(role)).with_context(|| {
            format!(
                "step {}: executor role:{role} has no team member: no bot holds role {role:?} \
                 (pass --team <team.yaml>, or add the role {role} to team.yaml)",
                step.id
            )
        })?;
        validate_executor(mapped).map_err(|e| {
            anyhow::anyhow!("step {}: role {role} maps to an invalid executor: {e}", step.id)
        })?;
        Ok(Some(mapped.clone()))
    }

    /// [`Template::expand_dag`] with a role map (role -> `bot:<slug>` /
    /// `ao:<harness>`, from team.yaml) used to resolve `role:<role>` executors.
    pub fn expand_dag_with_roles(
        &self,
        root_node_id: &str,
        provided: &HashMap<String, String>,
        roles: Option<&RoleMap>,
    ) -> Result<DagTemplateExpansion> {
        self.validate()?;
        let params = self.resolve_params(provided)?;
        let params_map: HashMap<String, String> =
            params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut executors: HashMap<&str, Option<String>> = HashMap::new();
        for step in &self.steps {
            executors.insert(step.id.as_str(), self.resolve_step_executor(step, roles)?);
        }
        let max_rounds = self.effective_max_rounds().unwrap_or(DEFAULT_MAX_ROUNDS);

        let suffix = {
            let mut b = [0u8; 2];
            rand::thread_rng().fill_bytes(&mut b);
            hex::encode(b)
        };
        let nodes: BTreeMap<String, String> = self
            .steps
            .iter()
            .map(|s| (s.id.clone(), format!("{}-{}", s.id, suffix)))
            .collect();
        let id_map: HashMap<String, String> =
            nodes.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        let mut mutations = Vec::new();
        for step in &self.steps {
            let description = placeholders::rewrite_node_ids(
                &placeholders::render_params(&step.description, &params_map),
                &id_map,
            );
            mutations.push(DagMutation::CreateNode {
                node_id: nodes[&step.id].clone(),
                node_kind: "task".to_string(),
                title: placeholders::render_params(&step.title, &params_map),
                parent_node_id: Some(root_node_id.to_string()),
                execution_mode: "shared".to_string(),
                description: (!description.trim().is_empty()).then_some(description),
                executor: executors[step.id.as_str()].clone(),
            });
            if step.retry.is_some() {
                mutations.push(DagMutation::AddLabel {
                    node_id: nodes[&step.id].clone(),
                    label: RETRY_SAFE_LABEL.to_string(),
                });
            }
            if let Some(target) = &step.on_fail {
                mutations.push(DagMutation::AddLabel {
                    node_id: nodes[&step.id].clone(),
                    label: format!("{ON_FAIL_LABEL_PREFIX}{}", nodes[target]),
                });
                mutations.push(DagMutation::AddLabel {
                    node_id: nodes[&step.id].clone(),
                    label: format!("{MAX_ROUNDS_LABEL_PREFIX}{max_rounds}"),
                });
            }
        }
        for step in &self.steps {
            for blocker in &step.blocked_by {
                mutations.push(DagMutation::AddBlockedBy {
                    from_node_id: nodes[blocker].clone(),
                    to_node_id: nodes[&step.id].clone(),
                });
            }
        }
        // The plan root stands for "the whole flow is done": it is blocked by
        // every terminal step, so it only becomes READY (to verify and close)
        // after the flow finishes instead of showing up as ready work at t0.
        let blockers: HashSet<&str> = self
            .steps
            .iter()
            .flat_map(|s| s.blocked_by.iter().map(String::as_str))
            .collect();
        for step in self.steps.iter().filter(|s| !blockers.contains(s.id.as_str())) {
            mutations.push(DagMutation::AddBlockedBy {
                from_node_id: nodes[&step.id].clone(),
                to_node_id: root_node_id.to_string(),
            });
        }
        for step in &self.steps {
            if let Some(gate) = &step.wait_gate {
                let mut params: HashMap<String, serde_json::Value> = gate
                    .params
                    .iter()
                    .map(|(k, v)| {
                        let v = match v.as_str() {
                            Some(s) => serde_json::Value::String(placeholders::render_params(
                                s,
                                &params_map,
                            )),
                            None => v.clone(),
                        };
                        (k.clone(), v)
                    })
                    .collect();
                let evidence = gate
                    .evidence
                    .as_deref()
                    .map(|e| placeholders::render_params(e, &params_map))
                    .filter(|e| !e.trim().is_empty());
                let mut description = gate
                    .description
                    .as_deref()
                    .map(|d| placeholders::render_params(d, &params_map));
                if let Some(e) = &evidence {
                    params.insert(EVIDENCE_PARAM.to_string(), serde_json::Value::String(e.clone()));
                    let base = description
                        .unwrap_or_else(|| placeholders::render_params(&step.title, &params_map));
                    description = Some(format!("{base} (look at: {e})"));
                }
                mutations.push(DagMutation::AddWaitGate {
                    node_id: nodes[&step.id].clone(),
                    gate_id: None,
                    kind: gate.kind.clone(),
                    description,
                    params,
                });
            }
        }
        // Closure texts live on the plan root as state dimensions, so drive
        // reads them from the projected DAG (no side database).
        if let Some(closure) = &self.closure {
            for (dimension, text) in [
                (CLOSURE_SUCCESS_STATE, &closure.success),
                (CLOSURE_DEGRADED_STATE, &closure.degraded),
                (CLOSURE_FAILED_STATE, &closure.failed),
            ] {
                if let Some(text) = text {
                    mutations.push(DagMutation::SetState {
                        node_id: root_node_id.to_string(),
                        dimension: dimension.to_string(),
                        value: placeholders::render_params(text, &params_map),
                        reason: Some(format!("template {} closure", self.id)),
                    });
                }
            }
        }
        Ok(DagTemplateExpansion {
            nodes,
            mutations,
            params,
        })
    }
}

/// Transitive blocked_by predecessors of `step_id` among `steps`.
fn transitive_blockers(steps: &[TemplateStep], step_id: &str) -> HashSet<String> {
    let by_id: HashMap<&str, &TemplateStep> = steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut out = HashSet::new();
    let mut stack = vec![step_id.to_string()];
    while let Some(id) = stack.pop() {
        if let Some(step) = by_id.get(id.as_str()) {
            for b in &step.blocked_by {
                if out.insert(b.clone()) {
                    stack.push(b.clone());
                }
            }
        }
    }
    out
}

/// Instantiate `template` as a new WIH DAG through Gate 0: `plan_new`
/// (root node = the plan prompt) then one `plan_refine` delta carrying every
/// node, edge, and wait-gate mutation. The template is fully validated before
/// the plan is created, so a bad template leaves no orphan DAG behind.
pub async fn plan_from_template(
    gate: &Gate,
    template: &Template,
    params: &HashMap<String, String>,
    raw_text: Option<&str>,
    project_id: Option<String>,
) -> Result<TemplatePlanResult> {
    plan_from_template_with_origin(gate, template, params, raw_text, project_id, None).await
}

/// [`plan_from_template`] with an explicit prompt origin (see
/// [`Gate::plan_new_with_origin`]): the plan prompt is attributed to
/// `origin.actor`, and the instantiation delta is authored by it.
pub async fn plan_from_template_with_origin(
    gate: &Gate,
    template: &Template,
    params: &HashMap<String, String>,
    raw_text: Option<&str>,
    project_id: Option<String>,
    origin: Option<&PromptOrigin>,
) -> Result<TemplatePlanResult> {
    plan_from_template_with_roles(gate, template, params, raw_text, project_id, origin, None).await
}

/// [`plan_from_template_with_origin`] with a role map (role -> `bot:<slug>` /
/// `ao:<harness>`, built by the caller from team.yaml) that resolves the
/// template's `role:<role>` executors. An unmapped role is refused before
/// anything is created.
pub async fn plan_from_template_with_roles(
    gate: &Gate,
    template: &Template,
    params: &HashMap<String, String>,
    raw_text: Option<&str>,
    project_id: Option<String>,
    origin: Option<&PromptOrigin>,
    roles: Option<&RoleMap>,
) -> Result<TemplatePlanResult> {
    // Validate (params, steps, edges, placeholders, roles) before creating anything.
    template.expand_dag_with_roles("__root__", params, roles)?;
    let effective = template.resolve_params(params)?;
    let param_text = effective
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = match raw_text {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => {
            let mut t = format!("Template {}: {}", template.name, template.description);
            if !param_text.is_empty() {
                t.push_str(&format!(" ({param_text})"));
            }
            t
        }
    };
    let (prompt_id, dag_id, root_node_id) =
        gate.plan_new_with_origin(&text, project_id, origin).await?;
    let expansion = template.expand_dag_with_roles(&root_node_id, params, roles)?;
    let delta = format!(
        "instantiate template {} ({}){}",
        template.id,
        template.name,
        if param_text.is_empty() {
            String::new()
        } else {
            format!(" with {param_text}")
        }
    );
    let delta_id = gate
        .plan_refine(
            &dag_id,
            &delta,
            origin.map(|o| o.actor.id.as_str()).unwrap_or("template"),
            expansion.mutations,
        )
        .await?;
    Ok(TemplatePlanResult {
        template_id: template.id.clone(),
        prompt_id,
        dag_id,
        root_node_id,
        delta_id,
        nodes: expansion.nodes,
        params: expansion.params,
    })
}

#[derive(Deserialize)]
struct MarkdownFrontmatter {
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct MarkdownSpec {
    #[serde(default)]
    params: Vec<TemplateParam>,
    steps: Vec<TemplateStep>,
    #[serde(default)]
    max_rounds: Option<u32>,
    #[serde(default)]
    closure: Option<TemplateClosure>,
}

/// Parse a markdown template: YAML frontmatter (`name`, `description`) and
/// exactly one ```` ```yaml template-spec ```` fenced block.
pub fn parse_markdown_template(id: &str, raw: &str) -> Result<Template> {
    let mut lines = raw.lines();
    let first = lines.by_ref().find(|l| !l.trim().is_empty());
    if first.map(str::trim) != Some("---") {
        bail!("template {id}: markdown template must start with --- frontmatter");
    }
    let mut front = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        front.push_str(line);
        front.push('\n');
    }
    if !closed {
        bail!("template {id}: unterminated frontmatter");
    }
    let fm: MarkdownFrontmatter = serde_yaml::from_str(&front)
        .with_context(|| format!("template {id}: invalid frontmatter (needs name)"))?;

    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in lines {
        let trimmed = line.trim();
        match current.as_mut() {
            Some(buf) => {
                if trimmed == "```" {
                    blocks.push(current.take().unwrap_or_default());
                } else {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
            None => {
                if let Some(info) = trimmed.strip_prefix("```") {
                    let mut words = info.split_whitespace();
                    if words.next() == Some("yaml") && words.any(|w| w == "template-spec") {
                        current = Some(String::new());
                    }
                }
            }
        }
    }
    if current.is_some() {
        bail!("template {id}: unterminated ```yaml template-spec block");
    }
    if blocks.len() != 1 {
        bail!(
            "template {id}: expected exactly one ```yaml template-spec block, found {}",
            blocks.len()
        );
    }
    let spec: MarkdownSpec = serde_yaml::from_str(&blocks[0])
        .with_context(|| format!("template {id}: invalid template-spec yaml"))?;
    Ok(Template {
        id: id.to_string(),
        name: fm.name,
        description: fm.description,
        params: spec.params,
        steps: spec.steps,
        max_rounds: spec.max_rounds,
        closure: spec.closure,
        created_at: Utc::now(),
        builtin: false,
    })
}

/// Templates compiled into the engine, as `(id, markdown source)`. A
/// workspace file with the same id wins.
const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    ("build-check-prove", include_str!("../../templates/build-check-prove.md")),
    ("fact-check", include_str!("../../templates/fact-check.md")),
];

/// The built-in template `id`, if there is one.
pub fn builtin_template(id: &str) -> Option<Template> {
    let (id, raw) = BUILTIN_TEMPLATES.iter().find(|(i, _)| *i == id)?;
    let mut t = parse_markdown_template(id, raw)
        .unwrap_or_else(|e| panic!("built-in template {id} does not parse: {e:#}"));
    t.builtin = true;
    // Fixed timestamp: built-ins sort after workspace templates, stably.
    t.created_at = chrono::DateTime::<Utc>::UNIX_EPOCH;
    Some(t)
}

/// Every built-in template.
pub fn builtin_templates() -> Vec<Template> {
    BUILTIN_TEMPLATES
        .iter()
        .filter_map(|(id, _)| builtin_template(id))
        .collect()
}

/// Store for workflow templates: `<root>/.allternit/rails/templates`, with
/// the built-ins as a fallback. Reads never create the directory.
pub struct TemplateStore {
    templates_dir: PathBuf,
}

impl TemplateStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let templates_dir = root.join(TEMPLATE_DIR);
        Ok(Self { templates_dir })
    }

    /// Create a new JSON template.
    pub fn create(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        steps: Vec<TemplateStep>,
    ) -> Result<Template> {
        let id = generate_template_id();
        let template = Template {
            id: id.clone(),
            name: name.into(),
            description: description.into(),
            params: Vec::new(),
            steps,
            max_rounds: None,
            closure: None,
            created_at: Utc::now(),
            builtin: false,
        };
        self.write(&template)?;
        Ok(template)
    }

    /// Load a template by ID: `<id>.json`, else `<id>.md`, else the built-in
    /// of that id.
    pub fn get(&self, id: &str) -> Result<Option<Template>> {
        if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
            bail!("invalid template id {id:?}");
        }
        let json = self.path(id);
        if json.exists() {
            return read_json(&json).with_context(|| format!("failed to read template {id}"));
        }
        let md = self.templates_dir.join(format!("{id}.md"));
        if md.exists() {
            return Self::load_file(&md).map(Some);
        }
        Ok(builtin_template(id))
    }

    /// Load a template file (`.json` or `.md`); the id is the file stem.
    pub fn load_file(path: &Path) -> Result<Template> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read template {path:?}"))?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("template")
            .to_string();
        match path.extension().and_then(|e| e.to_str()) {
            Some("md") | Some("markdown") => {
                let mut t = parse_markdown_template(&stem, &raw)?;
                if let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) {
                    t.created_at = chrono::DateTime::<Utc>::from(modified);
                }
                Ok(t)
            }
            _ => serde_json::from_str(&raw)
                .with_context(|| format!("failed to parse template {path:?}")),
        }
    }

    /// Resolve a template reference: an existing file path, else a store id.
    pub fn resolve(&self, id_or_path: &str) -> Result<Template> {
        let p = Path::new(id_or_path);
        if p.is_file() {
            return Self::load_file(p);
        }
        self.get(id_or_path)?
            .with_context(|| format!("template {id_or_path} not found in {}", self.templates_dir.display()))
    }

    /// List all templates (JSON and markdown), then the built-ins that no
    /// workspace file overrides (`builtin: true`).
    pub fn list(&self) -> Result<Vec<Template>> {
        let mut templates = self.list_files()?;
        let ids: HashSet<String> = templates.iter().map(|t| t.id.clone()).collect();
        templates.extend(builtin_templates().into_iter().filter(|t| !ids.contains(&t.id)));
        Ok(templates)
    }

    fn list_files(&self) -> Result<Vec<Template>> {
        let mut templates = Vec::new();
        if !self.templates_dir.is_dir() {
            return Ok(templates);
        }
        for entry in std::fs::read_dir(&self.templates_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("json") => {
                    if let Some(template) = read_json::<Template>(&path)? {
                        templates.push(template);
                    }
                }
                Some("md") => templates.push(Self::load_file(&path)?),
                _ => {}
            }
        }
        templates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(templates)
    }

    /// Delete a template (JSON or markdown).
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut deleted = false;
        for path in [self.path(id), self.templates_dir.join(format!("{id}.md"))] {
            if path.exists() {
                std::fs::remove_file(&path)?;
                deleted = true;
            }
        }
        Ok(deleted)
    }

    /// Instantiate a template into concrete tickets.
    pub fn instantiate(
        &self,
        id: &str,
        ticket_store: &TicketStore,
        dep_graph: &mut DependencyGraph,
    ) -> Result<InstantiationResult> {
        let template = self
            .get(id)?
            .with_context(|| format!("template {id} not found"))?;

        let root_id = TicketId::mint(template.name.as_bytes());

        // Create a mapping from template step id to generated ticket id.
        let mut step_to_ticket: HashMap<String, TicketId> = HashMap::new();
        let mut tickets = Vec::new();

        for step in &template.steps {
            let ticket_id = TicketId::mint(format!("{}.{}", root_id, step.id).as_bytes());
            step_to_ticket.insert(step.id.clone(), ticket_id.clone());

            let hierarchical_id = HierarchicalId::root(root_id.clone()).child(tickets.len() as u32 + 1);
            let ticket = Ticket {
                id: ticket_id,
                hierarchical_id,
                title: step.title.clone(),
                description: step.description.clone(),
                design: None,
                acceptance: None,
                notes: Vec::new(),
                status: TicketStatus::Open,
                kind: step.kind,
                priority: step.priority,
                assignee: None,
                estimate_minutes: None,
                due_at: None,
                defer_until: None,
                labels: vec![format!("template:{}", template.id)],
                external_ref: None,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("template_id".to_string(), serde_json::json!(template.id));
                    m.insert("template_step_id".to_string(), serde_json::json!(step.id));
                    m
                },
                created_at: Utc::now(),
                updated_at: Utc::now(),
                closed_at: None,
                close_reason: None,
            };
            tickets.push(ticket_store.create(ticket)?);
        }

        // Add dependencies between instantiated tickets.
        for step in &template.steps {
            let to_id = step_to_ticket
                .get(&step.id)
                .cloned()
                .context("missing ticket for step")?;
            for blocker_step_id in &step.blocked_by {
                let from_id = step_to_ticket
                    .get(blocker_step_id)
                    .cloned()
                    .with_context(|| format!("template step {blocker_step_id} not found"))?;
                let edge = DependencyEdge::new(from_id, to_id.clone(), DependencyKind::Blocks);
                if dep_graph.would_cycle(&edge) {
                    anyhow::bail!("template instantiation would create a cycle");
                }
                dep_graph.add(edge);
            }
        }

        Ok(InstantiationResult {
            template_id: template.id,
            root_id,
            tickets,
        })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.templates_dir.join(format!("{}.json", id))
    }

    fn write(&self, template: &Template) -> Result<()> {
        ensure_dir(&self.templates_dir)?;
        let path = self.path(&template.id);
        write_json_atomic(&path, template)
            .with_context(|| format!("failed to write template {path:?}"))
    }
}

fn generate_template_id() -> String {
    let mut nonce = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut nonce);
    format!("tmpl-{}-{}", Utc::now().timestamp_millis(), hex::encode(nonce))
}

/// Parse `k=v` CLI params.
pub fn parse_param_args(args: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for a in args {
        let (k, v) = a
            .split_once('=')
            .with_context(|| format!("--param {a:?} must be <name>=<value>"))?;
        if k.trim().is_empty() {
            bail!("--param {a:?} has an empty name");
        }
        out.insert(k.trim().to_string(), v.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const MD: &str = r#"---
name: Motion promo
description: capture, cut, review, re-record
---

Some prose that is ignored.

```yaml template-spec
params:
  - name: topic
  - name: length
    default: "30s"
steps:
  - id: capture
    title: "Capture {{ params.topic }}"
    description: "Record raw footage of {{ params.topic }}"
    executor: "ao:claude"
  - id: cut
    title: Cut
    description: "Cut {{ params.length }} from {{ capture.output }}"
    blocked_by: [capture]
  - id: review
    title: Gate-2 review
    blocked_by: [cut]
    wait_gate:
      kind: manual
      description: "Eoj reviews the cut"
  - id: rerecord
    title: Re-record
    description: "Notes at {{ capture.output_path }}"
    blocked_by: [review]
```
"#;

    #[test]
    fn create_and_instantiate() {
        let tmp = TempDir::new().unwrap();
        let template_store = TemplateStore::new(tmp.path()).unwrap();
        let ticket_store = TicketStore::new(tmp.path()).unwrap();

        let template = template_store
            .create(
                "OAuth flow",
                "Add OAuth authentication",
                vec![
                    TemplateStep {
                        id: "setup".to_string(),
                        title: "Set up OAuth provider".to_string(),
                        description: "...".to_string(),
                        kind: TicketKind::Task,
                        priority: TicketPriority::P1,
                        blocked_by: vec![],
                        executor: None,
                        wait_gate: None,
                        retry: None,
                        on_fail: None,
                    },
                    TemplateStep {
                        id: "ui".to_string(),
                        title: "Build login UI".to_string(),
                        description: "...".to_string(),
                        kind: TicketKind::Task,
                        priority: TicketPriority::P2,
                        blocked_by: vec!["setup".to_string()],
                        executor: None,
                        wait_gate: None,
                        retry: None,
                        on_fail: None,
                    },
                ],
            )
            .unwrap();

        let mut graph = DependencyGraph::new();
        let result = template_store
            .instantiate(&template.id, &ticket_store, &mut graph)
            .unwrap();

        assert_eq!(result.tickets.len(), 2);
        assert!(!graph.has_cycle());
        assert_eq!(graph.edges().count(), 1);
    }

    #[test]
    fn legacy_json_without_new_fields_still_loads() {
        let tmp = TempDir::new().unwrap();
        let store = TemplateStore::new(tmp.path()).unwrap();
        let raw = r#"{"id":"old","name":"Old","description":"d","created_at":"2026-01-01T00:00:00Z",
            "steps":[{"id":"a","title":"A","description":"x","kind":"task","priority":"p2","blocked_by":[]}]}"#;
        std::fs::create_dir_all(tmp.path().join(TEMPLATE_DIR)).unwrap();
        std::fs::write(tmp.path().join(TEMPLATE_DIR).join("old.json"), raw).unwrap();
        let t = store.get("old").unwrap().unwrap();
        assert_eq!(t.steps.len(), 1);
        assert!(t.params.is_empty());
        assert!(t.max_rounds.is_none() && t.closure.is_none() && !t.builtin);
        assert!(t.steps[0].on_fail.is_none());
    }

    #[test]
    fn markdown_template_parses_and_lists() {
        let tmp = TempDir::new().unwrap();
        let store = TemplateStore::new(tmp.path()).unwrap();
        std::fs::create_dir_all(tmp.path().join(TEMPLATE_DIR)).unwrap();
        std::fs::write(tmp.path().join(TEMPLATE_DIR).join("promo.md"), MD).unwrap();
        let t = store.get("promo").unwrap().unwrap();
        assert_eq!(t.id, "promo");
        assert_eq!(t.name, "Motion promo");
        assert_eq!(t.steps.len(), 4);
        assert_eq!(t.steps[2].wait_gate.as_ref().unwrap().kind, WaitGateKind::Manual);
        // The workspace file plus the two built-ins.
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1 + BUILTIN_TEMPLATES.len());
        assert_eq!(list[0].id, "promo");
        assert!(!list[0].builtin && list[1..].iter().all(|t| t.builtin));
        assert!(store.resolve("promo").is_ok());
    }

    #[test]
    fn markdown_requires_exactly_one_spec_block() {
        let two = format!("{MD}\n```yaml template-spec\nsteps: []\n```\n");
        assert!(parse_markdown_template("x", &two).is_err());
        let none = "---\nname: x\n---\nno block\n";
        assert!(parse_markdown_template("x", none).is_err());
    }

    #[test]
    fn missing_required_param_rejected() {
        let t = parse_markdown_template("promo", MD).unwrap();
        let err = t.expand_dag("root", &HashMap::new()).unwrap_err().to_string();
        assert!(err.contains("missing required param"), "{err}");
        assert!(err.contains("topic"), "{err}");
    }

    #[test]
    fn unknown_param_rejected() {
        let t = parse_markdown_template("promo", MD).unwrap();
        let mut p = HashMap::new();
        p.insert("topic".to_string(), "Projects".to_string());
        p.insert("tpoic".to_string(), "typo".to_string());
        assert!(t.expand_dag("root", &p).is_err());
    }

    #[test]
    fn expand_dag_renders_params_rewrites_refs_and_edges() {
        let t = parse_markdown_template("promo", MD).unwrap();
        let mut p = HashMap::new();
        p.insert("topic".to_string(), "Projects".to_string());
        let x = t.expand_dag("root", &p).unwrap();
        assert_eq!(x.params.get("length").map(String::as_str), Some("30s"));
        let capture = &x.nodes["capture"];
        let cut = &x.nodes["cut"];
        let mut creates = 0;
        let mut edges = Vec::new();
        let mut gates = 0;
        for m in &x.mutations {
            match m {
                DagMutation::CreateNode {
                    node_id,
                    title,
                    description,
                    executor,
                    parent_node_id,
                    ..
                } => {
                    creates += 1;
                    assert_eq!(parent_node_id.as_deref(), Some("root"));
                    if node_id == capture {
                        assert_eq!(title, "Capture Projects");
                        assert_eq!(executor.as_deref(), Some("ao:claude"));
                    }
                    if node_id == cut {
                        assert_eq!(
                            description.as_deref(),
                            Some(format!("Cut 30s from {{{{ {capture}.output }}}}").as_str())
                        );
                    }
                }
                DagMutation::AddBlockedBy {
                    from_node_id,
                    to_node_id,
                } => edges.push((from_node_id.clone(), to_node_id.clone())),
                DagMutation::AddWaitGate { node_id, kind, .. } => {
                    gates += 1;
                    assert_eq!(node_id, &x.nodes["review"]);
                    assert_eq!(kind, &WaitGateKind::Manual);
                }
                _ => {}
            }
        }
        assert_eq!(creates, 4);
        assert_eq!(gates, 1);
        assert!(edges.contains(&(capture.clone(), cut.clone())));
        // 3 step edges + the terminal step (rerecord) blocking the root.
        assert_eq!(edges.len(), 4);
        assert!(edges.contains(&(x.nodes["rerecord"].clone(), "root".to_string())));
    }

    #[test]
    fn expand_dag_rejects_ref_to_non_predecessor() {
        let md = MD.replace(
            "description: \"Notes at {{ capture.output_path }}\"",
            "description: \"Notes at {{ capture.output_path }}\"\n  - id: stray\n    title: Stray\n    description: \"{{ cut.output }}\"",
        );
        let t = parse_markdown_template("promo", &md).unwrap();
        let mut p = HashMap::new();
        p.insert("topic".to_string(), "x".to_string());
        let err = t.expand_dag("root", &p).unwrap_err().to_string();
        assert!(err.contains("not a blocked_by predecessor"), "{err}");
    }

    #[test]
    fn expand_dag_rejects_bad_executor_and_cycles() {
        let mut t = parse_markdown_template("promo", MD).unwrap();
        let mut p = HashMap::new();
        p.insert("topic".to_string(), "x".to_string());
        t.steps[0].executor = Some("human".to_string());
        assert!(t.expand_dag("root", &p).is_err());
        t.steps[0].executor = None;
        t.steps[0].blocked_by = vec!["rerecord".to_string()];
        assert!(t.expand_dag("root", &p).unwrap_err().to_string().contains("cycle"));
    }

    const LOOP_MD: &str = r#"---
name: Loop
description: build and check with a route back
---

```yaml template-spec
params:
  - name: intent
max_rounds: 4
steps:
  - id: build
    title: "Build {{ params.intent }}"
    executor: role:build
  - id: lint
    title: Lint
    executor: "ao:kimi"
    blocked_by: [build]
  - id: check
    title: Check
    executor: role:check
    blocked_by: [lint]
    on_fail: build
  - id: signoff
    title: Sign off
    blocked_by: [check]
    wait_gate:
      kind: manual
      evidence: "PROOF.md for {{ params.intent }}"
closure:
  success: "Proven: {{ params.intent }}"
  degraded: Stopped at max_rounds.
  failed: Could not be fixed.
```
"#;

    fn roles() -> RoleMap {
        let mut r = RoleMap::new();
        r.insert("build".into(), "bot:builder".into());
        r.insert("check".into(), "ao:codex".into());
        r
    }

    fn intent(v: &str) -> HashMap<String, String> {
        HashMap::from([("intent".to_string(), v.to_string())])
    }

    #[test]
    fn markdown_parses_on_fail_evidence_role_max_rounds_closure() {
        let t = parse_markdown_template("loop", LOOP_MD).unwrap();
        assert_eq!(t.max_rounds, Some(4));
        assert_eq!(t.effective_max_rounds(), Some(4));
        assert_eq!(t.steps[2].on_fail.as_deref(), Some("build"));
        assert_eq!(t.steps[0].executor.as_deref(), Some("role:build"));
        let g = t.steps[3].wait_gate.as_ref().unwrap();
        assert_eq!(g.evidence.as_deref(), Some("PROOF.md for {{ params.intent }}"));
        let c = t.closure.as_ref().unwrap();
        assert_eq!(c.degraded.as_deref(), Some("Stopped at max_rounds."));
        assert_eq!(c.failed.as_deref(), Some("Could not be fixed."));
        t.validate().unwrap();
    }

    #[test]
    fn json_parses_new_fields_and_round_trips_snake_case() {
        let raw = r#"{"id":"j","name":"J","created_at":"2026-01-01T00:00:00Z","max_rounds":2,
            "closure":{"degraded":"d"},
            "steps":[{"id":"a","title":"A","executor":"role:build"},
                     {"id":"b","title":"B","blocked_by":["a"],"on_fail":"a",
                      "wait_gate":{"kind":"manual","evidence":"proof/"}}]}"#;
        let t: Template = serde_json::from_str(raw).unwrap();
        assert_eq!(t.max_rounds, Some(2));
        assert_eq!(t.steps[1].on_fail.as_deref(), Some("a"));
        assert_eq!(t.steps[1].wait_gate.as_ref().unwrap().evidence.as_deref(), Some("proof/"));
        let back = serde_json::to_value(&t).unwrap();
        assert_eq!(back["max_rounds"], 2);
        assert_eq!(back["steps"][1]["on_fail"], "a");
        assert!(back.get("builtin").is_none());
        // `builtin` is never read from disk.
        let forged: Template =
            serde_json::from_str(&raw.replacen("\"id\":\"j\"", "\"id\":\"j\",\"builtin\":true", 1)).unwrap();
        assert!(!forged.builtin);
    }

    #[test]
    fn contract_json_matches_api_shape() {
        let t = parse_markdown_template("loop", LOOP_MD).unwrap();
        let c = t.to_contract_json();
        let keys = |v: &serde_json::Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(&c),
            ["closure", "description", "id", "maxRounds", "name", "params", "steps"]
        );
        assert_eq!(
            keys(&c["steps"][0]),
            ["blockedBy", "executor", "id", "onFail", "retry", "title", "waitGate"]
        );
        assert_eq!(c["maxRounds"], 4);
        assert_eq!(c["steps"][2]["onFail"], "build");
        assert_eq!(c["steps"][2]["blockedBy"], serde_json::json!(["lint"]));
        assert_eq!(c["steps"][0]["waitGate"], serde_json::Value::Null);
        assert_eq!(c["steps"][3]["waitGate"]["kind"], "manual");
        assert_eq!(c["steps"][3]["waitGate"]["evidence"], "PROOF.md for {{ params.intent }}");
        assert_eq!(c["closure"]["degraded"], "Stopped at max_rounds.");
        assert_eq!(c["params"][0], serde_json::json!({ "name": "intent" }));
        // No on_fail, no max_rounds -> null.
        let plain = parse_markdown_template("promo", MD).unwrap().to_contract_json();
        assert_eq!(plain["maxRounds"], serde_json::Value::Null);
        assert_eq!(plain["closure"], serde_json::Value::Null);
    }

    #[test]
    fn max_rounds_defaults_to_three_with_on_fail_and_rejects_zero() {
        let mut t = parse_markdown_template("loop", LOOP_MD).unwrap();
        t.max_rounds = None;
        assert_eq!(t.effective_max_rounds(), Some(DEFAULT_MAX_ROUNDS));
        let x = t.expand_dag_with_roles("root", &intent("x"), Some(&roles())).unwrap();
        let check = &x.nodes["check"];
        assert!(x.mutations.iter().any(|m| matches!(m,
            DagMutation::AddLabel { node_id, label } if node_id == check && label == "max_rounds:3")));
        t.max_rounds = Some(0);
        let err = t.validate().unwrap_err().to_string();
        assert!(err.contains("max_rounds must be a positive integer"), "{err}");
    }

    #[test]
    fn role_executors_resolve_through_the_role_map() {
        let t = parse_markdown_template("loop", LOOP_MD).unwrap();
        let x = t.expand_dag_with_roles("root", &intent("x"), Some(&roles())).unwrap();
        let mut seen = 0;
        for m in &x.mutations {
            if let DagMutation::CreateNode { node_id, executor, .. } = m {
                let want = if node_id == &x.nodes["build"] {
                    Some("bot:builder")
                } else if node_id == &x.nodes["check"] {
                    Some("ao:codex")
                } else if node_id == &x.nodes["lint"] {
                    Some("ao:kimi")
                } else {
                    None
                };
                assert_eq!(executor.as_deref(), want, "{node_id}");
                // Nodes never carry role: executors.
                assert!(executor.as_deref().map_or(true, |e| validate_executor(e).is_ok()));
                seen += 1;
            }
        }
        assert_eq!(seen, 4);
    }

    #[test]
    fn unmapped_role_is_an_error_naming_the_role() {
        let t = parse_markdown_template("loop", LOOP_MD).unwrap();
        let err = t.expand_dag("root", &intent("x")).unwrap_err().to_string();
        assert!(err.contains("role:build") && err.contains("--team"), "{err}");
        let mut partial = RoleMap::new();
        partial.insert("build".into(), "bot:builder".into());
        let err = t
            .expand_dag_with_roles("root", &intent("x"), Some(&partial))
            .unwrap_err()
            .to_string();
        assert!(err.contains("role check") || err.contains("role:check"), "{err}");
        assert!(err.contains("team.yaml"), "{err}");
        // A role may not map to another role.
        let mut bad = roles();
        bad.insert("check".into(), "role:build".into());
        let err = t.expand_dag_with_roles("root", &intent("x"), Some(&bad)).unwrap_err().to_string();
        assert!(err.contains("invalid executor"), "{err}");
    }

    #[test]
    fn on_fail_must_name_an_existing_predecessor() {
        let mut t = parse_markdown_template("loop", LOOP_MD).unwrap();
        t.steps[2].on_fail = Some("nope".into());
        let err = t.validate().unwrap_err().to_string();
        assert!(err.contains("unknown step nope"), "{err}");
        // signoff comes after check: not a predecessor.
        t.steps[2].on_fail = Some("signoff".into());
        let err = t.validate().unwrap_err().to_string();
        assert!(err.contains("must be a blocked_by predecessor"), "{err}");
        // Itself is not a predecessor either.
        t.steps[2].on_fail = Some("check".into());
        assert!(t.validate().is_err());
        // A transitive predecessor (build via lint) is fine.
        t.steps[2].on_fail = Some("build".into());
        t.validate().unwrap();
    }

    #[test]
    fn expansion_labels_on_fail_closure_and_evidence() {
        let t = parse_markdown_template("loop", LOOP_MD).unwrap();
        let x = t.expand_dag_with_roles("root", &intent("login"), Some(&roles())).unwrap();
        let (build, check, signoff) = (&x.nodes["build"], &x.nodes["check"], &x.nodes["signoff"]);
        let labels: Vec<(&String, &String)> = x
            .mutations
            .iter()
            .filter_map(|m| match m {
                DagMutation::AddLabel { node_id, label } => Some((node_id, label)),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&(check, &format!("on_fail:{build}"))));
        assert!(labels.contains(&(check, &"max_rounds:4".to_string())));
        assert_eq!(labels.len(), 2, "only the on_fail step is labelled");
        let states: BTreeMap<&str, &str> = x
            .mutations
            .iter()
            .filter_map(|m| match m {
                DagMutation::SetState { node_id, dimension, value, .. } if node_id == "root" => {
                    Some((dimension.as_str(), value.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(states.get(CLOSURE_SUCCESS_STATE), Some(&"Proven: login"));
        assert_eq!(states.get(CLOSURE_DEGRADED_STATE), Some(&"Stopped at max_rounds."));
        assert_eq!(states.get(CLOSURE_FAILED_STATE), Some(&"Could not be fixed."));
        let gate = x
            .mutations
            .iter()
            .find_map(|m| match m {
                DagMutation::AddWaitGate { node_id, description, params, .. } if node_id == signoff => {
                    Some((description.clone(), params.clone()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(gate.1[EVIDENCE_PARAM], "PROOF.md for login");
        assert_eq!(gate.0.as_deref(), Some("Sign off (look at: PROOF.md for login)"));
    }

    #[test]
    fn builtins_load_expand_and_yield_to_workspace_files() {
        let tmp = TempDir::new().unwrap();
        let store = TemplateStore::new(tmp.path()).unwrap();
        // Reads never create the template directory.
        assert!(!tmp.path().join(TEMPLATE_DIR).exists());
        let list = store.list().unwrap();
        assert_eq!(
            list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["build-check-prove", "fact-check"]
        );
        assert!(list.iter().all(|t| t.builtin));
        assert!(!tmp.path().join(TEMPLATE_DIR).exists());

        let bcp = store.resolve("build-check-prove").unwrap();
        assert!(bcp.builtin);
        assert_eq!(bcp.effective_max_rounds(), Some(3));
        assert_eq!(bcp.steps.iter().find(|s| s.id == "check").unwrap().on_fail.as_deref(), Some("build"));
        let prove = bcp.steps.iter().find(|s| s.id == "prove").unwrap();
        assert_eq!(prove.wait_gate.as_ref().unwrap().evidence.as_deref(), Some("PROOF.md and proof/ files"));
        assert!(bcp.closure.as_ref().unwrap().degraded.is_some());
        let x = bcp.expand_dag_with_roles("root", &intent("a login page"), Some(&roles())).unwrap();
        assert_eq!(x.nodes.len(), 3);
        // Without a team the role executors are refused, naming the role.
        let err = bcp.expand_dag("root", &intent("x")).unwrap_err().to_string();
        assert!(err.contains("role:build"), "{err}");

        let fc = store.get("fact-check").unwrap().unwrap();
        assert_eq!(fc.effective_max_rounds(), Some(2));
        assert_eq!(fc.steps.iter().find(|s| s.id == "verify").unwrap().on_fail.as_deref(), Some("research"));
        let mut fr = RoleMap::new();
        fr.insert("research".into(), "bot:research".into());
        fr.insert("check".into(), "bot:checker".into());
        let claim = HashMap::from([("claim".to_string(), "Water boils at 100 C at sea level".to_string())]);
        let x = fc.expand_dag_with_roles("root", &claim, Some(&fr)).unwrap();
        assert_eq!(x.params.get("source").map(String::as_str), Some("any"));
        assert!(fc.expand_dag_with_roles("root", &HashMap::new(), Some(&fr)).is_err());

        // A workspace file with the same id wins.
        std::fs::create_dir_all(tmp.path().join(TEMPLATE_DIR)).unwrap();
        std::fs::write(tmp.path().join(TEMPLATE_DIR).join("fact-check.md"), MD).unwrap();
        let own = store.get("fact-check").unwrap().unwrap();
        assert!(!own.builtin);
        assert_eq!(own.name, "Motion promo");
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.iter().filter(|t| t.builtin).count(), 1);
        // Built-ins are not deletable files.
        assert!(!store.delete("build-check-prove").unwrap());
    }

    #[test]
    fn parse_param_args_splits_on_first_equals() {
        let p = parse_param_args(&["a=b=c".to_string()]).unwrap();
        assert_eq!(p["a"], "b=c");
        assert!(parse_param_args(&["nope".to_string()]).is_err());
    }
}
