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
//! - `<id>.json` — a serialized [`Template`] (what `commrails template new`
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
use crate::gate::gate::DagMutation;
use crate::gate::Gate;
use crate::rails_id::{HierarchicalId, TicketId};
use crate::tickets::{Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore};
use crate::wait_gates::WaitGateKind;
use crate::work::placeholders;
use crate::work::types::{validate_executor, DagEdge};

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
    /// `bot:<slug>` | `ao:<harness>` (WIH DAG only; recorded, not acted on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// Wait-gate attached to the node (WIH DAG only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_gate: Option<TemplateWaitGate>,
}

/// A wait-gate declared on a template step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateWaitGate {
    pub kind: WaitGateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    #[serde(default = "Utc::now")]
    pub created_at: chrono::DateTime<Utc>,
}

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
                for v in gate.params.values() {
                    if let Some(s) = v.as_str() {
                        out.extend(placeholders::param_refs(s));
                    }
                }
            }
        }
        out
    }

    /// Validate the template and expand it into DAG mutations whose nodes are
    /// children of `root_node_id`. Node ids are `<step_id>-<suffix>` with one
    /// random suffix per instantiation (node ids must be unique across dags).
    pub fn expand_dag(
        &self,
        root_node_id: &str,
        provided: &HashMap<String, String>,
    ) -> Result<DagTemplateExpansion> {
        let params = self.resolve_params(provided)?;
        let params_map: HashMap<String, String> =
            params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
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
        for step in &self.steps {
            if let Some(executor) = &step.executor {
                validate_executor(executor)
                    .map_err(|e| anyhow::anyhow!("step {}: {}", step.id, e))?;
            }
            let preds = transitive_blockers(&self.steps, &step.id);
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
                executor: step.executor.clone(),
            });
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
                let params = gate
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
                mutations.push(DagMutation::AddWaitGate {
                    node_id: nodes[&step.id].clone(),
                    gate_id: None,
                    kind: gate.kind.clone(),
                    description: gate
                        .description
                        .as_deref()
                        .map(|d| placeholders::render_params(d, &params_map)),
                    params,
                });
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
    // Validate (params, steps, edges, placeholders) before creating anything.
    template.expand_dag("__root__", params)?;
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
    let (prompt_id, dag_id, root_node_id) = gate.plan_new(&text, project_id).await?;
    let expansion = template.expand_dag(&root_node_id, params)?;
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
        .plan_refine(&dag_id, &delta, "template", expansion.mutations)
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
        created_at: Utc::now(),
    })
}

/// Store for workflow templates.
pub struct TemplateStore {
    templates_dir: PathBuf,
}

impl TemplateStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let templates_dir = root.join(TEMPLATE_DIR);
        ensure_dir(&templates_dir)?;
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
            created_at: Utc::now(),
        };
        self.write(&template)?;
        Ok(template)
    }

    /// Load a template by ID: `<id>.json`, else `<id>.md`.
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
        Ok(None)
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

    /// List all templates (JSON and markdown).
    pub fn list(&self) -> Result<Vec<Template>> {
        let mut templates = Vec::new();
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
        std::fs::write(tmp.path().join(TEMPLATE_DIR).join("old.json"), raw).unwrap();
        let t = store.get("old").unwrap().unwrap();
        assert_eq!(t.steps.len(), 1);
        assert!(t.params.is_empty());
    }

    #[test]
    fn markdown_template_parses_and_lists() {
        let tmp = TempDir::new().unwrap();
        let store = TemplateStore::new(tmp.path()).unwrap();
        std::fs::write(tmp.path().join(TEMPLATE_DIR).join("promo.md"), MD).unwrap();
        let t = store.get("promo").unwrap().unwrap();
        assert_eq!(t.id, "promo");
        assert_eq!(t.name, "Motion promo");
        assert_eq!(t.steps.len(), 4);
        assert_eq!(t.steps[2].wait_gate.as_ref().unwrap().kind, WaitGateKind::Manual);
        assert_eq!(store.list().unwrap().len(), 1);
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

    #[test]
    fn parse_param_args_splits_on_first_equals() {
        let p = parse_param_args(&["a=b=c".to_string()]).unwrap();
        assert_eq!(p["a"], "b=c");
        assert!(parse_param_args(&["nope".to_string()]).is_err());
    }
}
