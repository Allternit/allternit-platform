use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::verification::types::{ProviderError, VerificationProvider, VisualConfig};

use crate::gate::errors::{gate_ids, GateError};
use crate::wait_gates::{GateOutcome, WaitGateKind};
use crate::fence::Fence;
use crate::work::placeholders::{self, PlaceholderField};
use std::collections::HashMap;

use crate::core::ids::{create_event_id, create_lease_id, create_receipt_id};
use crate::core::io::{ensure_dir, write_json_atomic};
use crate::core::types::{
    AllternitEvent, Actor, ActorType, EventScope, LeaseRequest, LedgerQuery, ReceiptRecord,
};
use crate::index::Index;
use crate::leases::Leases;
use crate::ledger::Ledger;
use crate::policy;
use crate::projections::views::{
    write_dag_view, write_elicitation_view, write_prompt_view, write_sampling_view, write_wih_view,
};
use crate::prompt::project_prompt;
use crate::receipts::ReceiptStore;
use crate::vault::Vault;
use crate::wih::projection::project_wih;
use crate::wih::types::LoopPolicy;
use crate::work::graph::{would_create_cycle, would_create_parent_cycle};
use crate::work::projection::project_dag;
use crate::work::types::{DagEdge, DagNode, DagRelation, DagState};

// Fail-closed judge wiring (Gate 2 judge step, Gate 4 verdict, verifier-only
// close, lease heartbeats/reclaim). Child module so it can reach the Gate's
// private stores; see `gate_judge.rs` and `spec/JUDGE.md`.
#[path = "gate_judge.rs"]
mod gate_judge;
pub use gate_judge::{CloseOutcome, HumanDecision, JudgeHandle, ReclaimRecord};

#[derive(Clone)]
pub struct GateOptions {
    pub ledger: Arc<Ledger>,
    pub leases: Arc<Leases>,
    pub receipts: Arc<ReceiptStore>,
    pub index: Option<Arc<Index>>,
    pub vault: Option<Arc<Vault>>,
    pub oauth_vault: Option<Arc<crate::vault::OAuthVault>>,
    pub root_dir: Option<PathBuf>,
    pub actor_id: Option<String>,
    pub strict_provenance: Option<bool>,
    /// Optional visual verification provider
    pub visual_provider: Option<Arc<dyn VerificationProvider>>,
    /// Visual verification configuration
    pub visual_config: Option<VisualConfig>,
}

pub struct Gate {
    ledger: Arc<Ledger>,
    leases: Arc<Leases>,
    receipts: Arc<ReceiptStore>,
    index: Option<Arc<Index>>,
    vault: Option<Arc<Vault>>,
    oauth_vault: Option<Arc<crate::vault::OAuthVault>>,
    root_dir: PathBuf,
    actor_id: String,
    strict_provenance: bool,
    visual_provider: Option<Arc<dyn VerificationProvider>>,
    visual_config: Option<VisualConfig>,
    /// Installed judge (`with_judge`); None = load `.allternit/judge/config.json`.
    judge: Option<JudgeHandle>,
}

#[derive(Debug)]
pub struct GateResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WihPickupOptions {
    pub role: Option<String>,
    pub fresh: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MutationProvenance {
    pub prompt_id: Option<String>,
    pub delta_id: Option<String>,
    pub agent_decision_id: Option<String>,
}

/// Who submitted a plan's raw intent when it is not the local operator —
/// e.g. a scoped remote identity on the CommRails bridge. See
/// [`Gate::plan_new_with_origin`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptOrigin {
    /// Recorded as the `PromptCreated` event actor.
    pub actor: Actor,
    /// `PromptCreated.payload.source` (e.g. `"bridge"`).
    pub source: String,
    /// Caller-side decision reference (e.g. the remote agent's decision log
    /// id). Opaque to the gate; recorded verbatim.
    #[serde(default)]
    pub decision_ref: Option<String>,
    /// Transport request id, joining the plan with its request audit event.
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DagSlice {
    nodes: Vec<DagNode>,
    edges: Vec<DagEdge>,
    relations: Vec<DagRelation>,
}

#[derive(Debug, Clone, Serialize)]
struct ContextReceipt {
    receipt_id: String,
    wih_id: String,
    node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ContextPack {
    version: String,
    generated_at: String,
    wih_id: String,
    dag_id: String,
    node_id: String,
    execution_mode: String,
    prompt: Option<crate::prompt::PromptTimeline>,
    dag_slice: DagSlice,
    dependency_nodes: Vec<String>,
    receipts: Vec<ContextReceipt>,
    /// Recorded outputs of blocked_by predecessors (hard deps).
    dependency_outputs: Vec<ContextNodeOutput>,
    /// The node description with output placeholders resolved at pickup.
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_description: Option<String>,
    /// Per-render fence around untrusted inlined content (S7): every
    /// `dependency_outputs[].text` and every `{{ x.output }}` substitution in
    /// `resolved_description` is wrapped in `<untrusted-data nonce=…>`.
    untrusted_fence: ContextFence,
}

#[derive(Debug, Clone, Serialize)]
struct ContextFence {
    nonce: String,
    instruction: String,
}

/// Max bytes of a predecessor's output inlined into a ContextPack
/// (`dependency_outputs[].text`). Longer outputs are cut at a char boundary
/// and flagged `truncated: true`; the full text stays at `output_path`.
pub const CONTEXT_PACK_OUTPUT_INLINE_CAP: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize)]
struct ContextNodeOutput {
    node_id: String,
    wih_id: String,
    receipt_id: String,
    blob_id: String,
    sha256: String,
    size_bytes: u64,
    output_path: String,
    text: Option<String>,
    truncated: bool,
}

/// Result of a Gate 1 pickup.
#[derive(Debug, Clone, Serialize)]
pub struct WihPickup {
    pub wih_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_pack_path: Option<String>,
    /// Set when the node description had output placeholders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_prompt_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_description: Option<String>,
    /// Nonce of the untrusted-content fence used for this render (S7).
    pub fence_nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutolandImpact {
    pub modified: Vec<String>,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutolandResult {
    pub success: bool,
    pub dry_run: bool,
    pub impact: AutolandImpact,
    pub backup_dir: Option<String>,
}

impl Gate {
    pub fn new(opts: GateOptions) -> Self {
        Self {
            ledger: opts.ledger,
            leases: opts.leases,
            receipts: opts.receipts,
            index: opts.index,
            vault: opts.vault,
            oauth_vault: opts.oauth_vault,
            root_dir: opts
                .root_dir
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
            actor_id: opts.actor_id.unwrap_or_else(|| "gate".to_string()),
            strict_provenance: opts.strict_provenance.unwrap_or(true),
            visual_provider: opts.visual_provider,
            visual_config: opts.visual_config,
            judge: None,
        }
    }

    pub async fn plan_new(
        &self,
        raw_text: &str,
        project_id: Option<String>,
    ) -> Result<(String, String, String)> {
        self.plan_new_with_origin(raw_text, project_id, None).await
    }

    /// Gate 0 plan creation with an explicit prompt origin. With `origin`,
    /// `PromptCreated` is attributed to the submitting actor (e.g. the bridge
    /// identity `bot:chief`) and its payload records `source`, `submitted_by`,
    /// `decision_ref` and `request_id`; the initial `PromptDeltaAppended` is
    /// authored by that actor. DAG mutations stay gate-emitted with
    /// prompt/delta provenance, so the plan's provenance chain ends at a
    /// prompt the remote actor owns (Gate 0: "prompt delta or agent decision").
    pub async fn plan_new_with_origin(
        &self,
        raw_text: &str,
        project_id: Option<String>,
        origin: Option<&PromptOrigin>,
    ) -> Result<(String, String, String)> {
        self.ensure_policy_scope(&EventScope::default()).await?;
        let prompt_id = format!("p_{}", rand::random::<u32>() % 1_000_000);
        let dag_id = format!("dag_{}", rand::random::<u32>() % 1_000_000);
        let node_id = format!("n_{}", rand::random::<u32>() % 10_000);
        let delta_id = format!("d_{}", rand::random::<u32>() % 1_000_000);

        let prompt_event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: origin
                .map(|o| o.actor.clone())
                .unwrap_or_else(|| gate_actor(&self.actor_id)),
            scope: project_id
                .clone()
                .map(|pid| crate::core::types::EventScope {
                    project_id: Some(pid),
                    ..Default::default()
                }),
            r#type: "PromptCreated".to_string(),
            payload: match origin {
                None => json!({
                    "prompt_id": prompt_id,
                    "source": "cli",
                    "raw_text": raw_text
                }),
                Some(o) => json!({
                    "prompt_id": prompt_id,
                    "source": o.source,
                    "raw_text": raw_text,
                    "submitted_by": o.actor.id,
                    "decision_ref": o.decision_ref,
                    "request_id": o.request_id
                }),
            },
            provenance: None,
        };
        self.emit(prompt_event).await?;

        let mutation_prov = MutationProvenance {
            prompt_id: Some(prompt_id.clone()),
            delta_id: Some(delta_id.clone()),
            agent_decision_id: None,
        };
        self.ensure_mutation_provenance(&mutation_prov)?;
        let dag_created = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagCreated".to_string(),
            payload: json!({ "dag_id": dag_id }),
            provenance: Some(self.provenance_from(&mutation_prov)),
        };
        let dag_created_id = dag_created.event_id.clone();
        self.emit(dag_created).await?;

        let node_created = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagNodeCreated".to_string(),
            payload: json!({
                "node_id": node_id,
                "dag_id": dag_id,
                "node_kind": "task",
                "title": raw_text.chars().take(120).collect::<String>(),
                "execution_mode": "shared"
            }),
            provenance: Some(self.provenance_from(&mutation_prov)),
        };
        let node_created_id = node_created.event_id.clone();
        self.emit(node_created).await?;

        let delta_event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "PromptDeltaAppended".to_string(),
            payload: json!({
                "prompt_id": prompt_id,
                "delta_id": delta_id,
                "author": origin.map(|o| o.actor.id.as_str()).unwrap_or("user"),
                "category": "initial",
                "delta_text": raw_text,
                "valid_at": Utc::now().to_rfc3339(),
                "links": { "mutations": [dag_created_id, node_created_id] }
            }),
            provenance: None,
        };
        self.emit(delta_event).await?;

        let link = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "PromptLinkedToWork".to_string(),
            payload: json!({ "prompt_id": prompt_id, "dag_id": dag_id }),
            provenance: None,
        };
        self.emit(link).await?;

        self.refresh_dag_view(&dag_id).await?;
        self.refresh_prompt_view(&prompt_id).await?;

        Ok((prompt_id, dag_id, node_id))
    }

    pub async fn plan_refine(
        &self,
        dag_id: &str,
        delta_text: &str,
        author: &str,
        mutations: Vec<DagMutation>,
    ) -> Result<String> {
        let dag_scope = EventScope {
            dag_id: Some(dag_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&dag_scope).await?;
        let prompt_id = self.resolve_prompt_id(dag_id).await?;
        let delta_id = format!("d_{}", rand::random::<u32>() % 1_000_000);
        let mut mutation_ids = Vec::new();
        let mutation_prov = MutationProvenance {
            prompt_id: Some(prompt_id.clone()),
            delta_id: Some(delta_id.clone()),
            agent_decision_id: None,
        };
        self.ensure_mutation_provenance(&mutation_prov)?;
        self.validate_mutations(dag_id, &mutations, Some(&prompt_id), Some(&delta_id))
            .await?;

        for mutation in mutations {
            let event = match mutation {
                DagMutation::CreateNode {
                    node_id,
                    node_kind,
                    title,
                    parent_node_id,
                    execution_mode,
                    description,
                    executor,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeCreated".to_string(),
                    payload: json!({
                        "node_id": node_id,
                        "dag_id": dag_id,
                        "node_kind": node_kind,
                        "title": title,
                        "parent_node_id": parent_node_id,
                        "execution_mode": execution_mode,
                        "description": description,
                        "executor": executor
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::UpdateNode { node_id, patch } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeUpdated".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "patch": patch
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::DeleteNode { node_id } => {
                    // Project from the full event log: DagNodeUpdated events carry no
                    // dag_id in their payload, so the dag-scoped event filter would
                    // miss renames and serve a stale title.
                    let all_events = self.ledger.query(LedgerQuery::default()).await?;
                    let dag = project_dag(&all_events, dag_id);
                    let (title, parent_node_id) = dag
                        .nodes
                        .get(&node_id)
                        .map(|n| (Some(n.title.clone()), n.parent_node_id.clone()))
                        .unwrap_or((None, None));
                    AllternitEvent {
                        event_id: create_event_id(),
                        ts: Utc::now().to_rfc3339(),
                        actor: gate_actor(&self.actor_id),
                        scope: None,
                        r#type: "DagNodeRemoved".to_string(),
                        payload: json!({
                            "dag_id": dag_id,
                            "node_id": node_id,
                            "title": title,
                            "parent_node_id": parent_node_id
                        }),
                        provenance: Some(self.provenance_from(&mutation_prov)),
                    }
                }
                DagMutation::ReparentNode {
                    node_id,
                    new_parent_id,
                } => {
                    // Project from the full event log: the dag-scoped event
                    // filter misses dag_id-less events (e.g. pre-v5
                    // WIHPickedUp), so a dag projected from filtered events
                    // can serve a stale parent chain to the cycle check.
                    let all_events = self.ledger.query(LedgerQuery::default()).await?;
                    let dag = project_dag(&all_events, dag_id);
                    let old_parent_id = dag
                        .nodes
                        .get(&node_id)
                        .ok_or_else(|| anyhow!("node not found: {}", node_id))?
                        .parent_node_id
                        .clone();
                    if let Some(parent) = &new_parent_id {
                        if !dag.nodes.contains_key(parent) {
                            return Err(anyhow!("new parent not found in dag: {}", parent));
                        }
                        if would_create_parent_cycle(&dag, &node_id, parent) {
                            return Err(anyhow!(
                                "reparent_node would create parent cycle: {} under {}",
                                node_id,
                                parent
                            ));
                        }
                    }
                    AllternitEvent {
                        event_id: create_event_id(),
                        ts: Utc::now().to_rfc3339(),
                        actor: gate_actor(&self.actor_id),
                        scope: None,
                        r#type: "DagNodeReparented".to_string(),
                        payload: json!({
                            "dag_id": dag_id,
                            "node_id": node_id,
                            "new_parent_id": new_parent_id,
                            "old_parent_id": old_parent_id
                        }),
                        provenance: Some(self.provenance_from(&mutation_prov)),
                    }
                }
                DagMutation::AddBlockedBy {
                    from_node_id,
                    to_node_id,
                } => {
                    self.ensure_no_cycle(dag_id, &from_node_id, &to_node_id)
                        .await?;
                    AllternitEvent {
                        event_id: create_event_id(),
                        ts: Utc::now().to_rfc3339(),
                        actor: gate_actor(&self.actor_id),
                        scope: None,
                        r#type: "DagEdgeAdded".to_string(),
                        payload: json!({
                            "dag_id": dag_id,
                            "from_node_id": from_node_id,
                            "to_node_id": to_node_id,
                            "edge_type": "blocked_by"
                        }),
                        provenance: Some(self.provenance_from(&mutation_prov)),
                    }
                }
                DagMutation::AddRelation {
                    a,
                    b,
                    note,
                    context_share,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagRelationAdded".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "a": a,
                        "b": b,
                        "note": note,
                        "context_share": context_share.unwrap_or(false)
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::AddLabel { node_id, label } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "LabelAdded".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "label": label
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::RemoveLabel { node_id, label } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "LabelRemoved".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "label": label
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::SetState {
                    node_id,
                    dimension,
                    value,
                    reason,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "StateSet".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "dimension": dimension,
                        "value": value,
                        "reason": reason
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::AddComment {
                    node_id,
                    body_ref,
                    author,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "CommentAdded".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "comment_id": create_event_id(),
                        "author": author,
                        "body_ref": body_ref
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
                DagMutation::AddWaitGate {
                    node_id,
                    gate_id,
                    kind,
                    description,
                    params,
                } => self.wait_gate_added_event(
                    dag_id,
                    &node_id,
                    gate_id,
                    kind,
                    description,
                    params,
                    Some(self.provenance_from(&mutation_prov)),
                ),
                DagMutation::ChangeStatus {
                    node_id,
                    from,
                    to,
                    reason,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeStatusChanged".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "from": from,
                        "to": to,
                        "reason": reason
                    }),
                    provenance: Some(self.provenance_from(&mutation_prov)),
                },
            };
            let event_id = event.event_id.clone();
            self.emit(event).await?;
            mutation_ids.push(event_id);
        }

        let links = mutation_ids.clone();
        let delta_event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "PromptDeltaAppended".to_string(),
            payload: json!({
                "prompt_id": prompt_id,
                "delta_id": delta_id,
                "author": author,
                "category": "refine",
                "delta_text": delta_text,
                "valid_at": Utc::now().to_rfc3339(),
                "links": { "mutations": links },
                "linked_mutations": mutation_ids
            }),
            provenance: None,
        };
        if self.strict_provenance && mutation_ids.is_empty() {
            return Err(anyhow!(
                "prompt delta requires linked mutations in strict mode"
            ));
        }
        self.emit(delta_event).await?;

        self.refresh_dag_view(dag_id).await?;
        self.refresh_prompt_view(&prompt_id).await?;

        Ok(delta_id)
    }

    pub async fn add_blocked_by(
        &self,
        dag_id: &str,
        from_node_id: &str,
        to_node_id: &str,
    ) -> Result<()> {
        self.add_blocked_by_with(
            dag_id,
            from_node_id,
            to_node_id,
            MutationProvenance::default(),
        )
        .await
    }

    pub async fn add_blocked_by_with(
        &self,
        dag_id: &str,
        from_node_id: &str,
        to_node_id: &str,
        provenance: MutationProvenance,
    ) -> Result<()> {
        self.ensure_mutation_provenance(&provenance)?;
        self.ensure_no_cycle(dag_id, from_node_id, to_node_id)
            .await?;
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagEdgeAdded".to_string(),
            payload: json!({
                "dag_id": dag_id,
                "from_node_id": from_node_id,
                "to_node_id": to_node_id,
                "edge_type": "blocked_by"
            }),
            provenance: Some(self.provenance_from(&provenance)),
        };
        if let Some(decision_id) = &provenance.agent_decision_id {
            if self.strict_provenance {
                self.ensure_agent_decision_links(decision_id, &evt.event_id)
                    .await?;
            }
        }
        self.emit(evt).await?;
        self.refresh_dag_view(dag_id).await?;
        Ok(())
    }

    pub async fn add_relation(
        &self,
        dag_id: &str,
        a: &str,
        b: &str,
        note: Option<String>,
    ) -> Result<()> {
        self.add_relation_with(dag_id, a, b, note, None, MutationProvenance::default())
            .await
    }

    pub async fn add_relation_with(
        &self,
        dag_id: &str,
        a: &str,
        b: &str,
        note: Option<String>,
        context_share: Option<bool>,
        provenance: MutationProvenance,
    ) -> Result<()> {
        self.ensure_mutation_provenance(&provenance)?;
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagRelationAdded".to_string(),
            payload: json!({
                "dag_id": dag_id,
                "a": a,
                "b": b,
                "note": note,
                "context_share": context_share.unwrap_or(false)
            }),
            provenance: Some(self.provenance_from(&provenance)),
        };
        if let Some(decision_id) = &provenance.agent_decision_id {
            if self.strict_provenance {
                self.ensure_agent_decision_links(decision_id, &evt.event_id)
                    .await?;
            }
        }
        self.emit(evt).await?;
        self.refresh_dag_view(dag_id).await?;
        Ok(())
    }

    pub async fn wih_pickup(&self, dag_id: &str, node_id: &str, agent_id: &str) -> Result<String> {
        self.wih_pickup_with(dag_id, node_id, agent_id, WihPickupOptions::default())
            .await
    }

    pub async fn wih_pickup_with(
        &self,
        dag_id: &str,
        node_id: &str,
        agent_id: &str,
        opts: WihPickupOptions,
    ) -> Result<String> {
        Ok(self
            .wih_pickup_detailed(dag_id, node_id, agent_id, opts)
            .await?
            .wih_id)
    }

    /// Gate 1. Refuses (with a structured [`GateError`]) unless the node is
    /// READY: blocked_by predecessors DONE, every wait-gate satisfied (elapsed
    /// timers are resolved lazily here), and every `{{ <node>.output }}`
    /// placeholder in its description points at a blocked_by predecessor
    /// (transitive) that has a recorded output.
    pub async fn wih_pickup_detailed(
        &self,
        dag_id: &str,
        node_id: &str,
        agent_id: &str,
        opts: WihPickupOptions,
    ) -> Result<WihPickup> {
        let scope = EventScope {
            dag_id: Some(dag_id.to_string()),
            node_id: Some(node_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        self.resolve_elapsed_timer_gates(dag_id).await?;
        let dag_events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&dag_events, dag_id);
        let node = dag
            .nodes
            .get(node_id)
            .ok_or_else(|| anyhow!("node not found"))?;
        if let Some(owner_role) = &node.owner_role {
            if opts.role.as_deref() != Some(owner_role.as_str()) {
                return Err(anyhow!("role does not match owner_role"));
            }
        }
        self.ensure_node_ready(&dag, node_id)?;
        if let Some(active) = self.active_wih_for_node(node_id).await? {
            return Err(anyhow!("node already has active wih {}", active));
        }
        // One fence per render (S7): minted after every predecessor output
        // it wraps was recorded, so that content cannot know the nonce.
        let fence = Fence::new();
        let resolved_description = self.resolve_node_description(&dag, node_id, &fence)?;

        let execution_mode = if opts.fresh {
            "fresh".to_string()
        } else {
            node.execution_mode.clone()
        };
        let wih_id = format!("wih_{}", rand::random::<u32>() % 10_000);
        let context_pack_path = if execution_mode == "fresh" {
            Some(
                context_pack_path(&self.root_dir, dag_id, &wih_id)
                    .to_string_lossy()
                    .to_string(),
            )
        } else {
            None
        };
        let resolved_prompt_path = match &resolved_description {
            Some(text) => {
                let path = resolved_prompt_path(&self.root_dir, dag_id, &wih_id);
                if let Some(parent) = path.parent() {
                    ensure_dir(parent)?;
                }
                std::fs::write(&path, text)?;
                Some(path.to_string_lossy().to_string())
            }
            None => None,
        };
        let template_refs: Vec<serde_json::Value> = node
            .description
            .as_deref()
            .map(placeholders::node_refs)
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let receipt_id = dag
                    .nodes
                    .get(&r.node_id)
                    .and_then(|n| n.output.as_ref())
                    .map(|o| o.receipt_id.clone());
                json!({ "node_id": r.node_id, "field": r.field.as_str(), "receipt_id": receipt_id })
            })
            .collect();
        let loop_policy = LoopPolicy::default();
        let created = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "WIHCreated".to_string(),
            payload: json!({
                "wih_id": wih_id,
                "dag_id": dag_id,
                "node_id": node_id,
                "execution_mode": execution_mode,
                "context_pack_path": context_pack_path,
                "resolved_prompt_path": resolved_prompt_path,
                "template_refs": template_refs,
                "policy": {
                    "requires_lease_for_write": true,
                    "loop_policy": serde_json::to_value(&loop_policy).unwrap_or(json!({}))
                }
            }),
            provenance: None,
        };
        self.emit(created).await?;

        let picked = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "WIHPickedUp".to_string(),
            payload: json!({
                "wih_id": wih_id,
                "dag_id": dag_id,
                "node_id": node_id,
                "agent_id": agent_id,
                "role": opts.role,
                "picked_up_at": Utc::now().to_rfc3339()
            }),
            provenance: None,
        };
        self.emit(picked).await?;

        if execution_mode == "fresh" {
            let _ = self
                .write_context_pack(&wih_id, dag_id, node_id, resolved_description.clone(), &fence)
                .await;
        }

        self.refresh_wih_view(&wih_id).await?;

        Ok(WihPickup {
            wih_id,
            context_pack_path,
            resolved_prompt_path,
            resolved_description,
            fence_nonce: fence.nonce().to_string(),
        })
    }

    /// Gate 1 readiness: status READY, blocked_by predecessors DONE, no
    /// unsatisfied wait-gate. Call after `resolve_elapsed_timer_gates`.
    fn ensure_node_ready(&self, dag: &DagState, node_id: &str) -> Result<()> {
        let node = dag
            .nodes
            .get(node_id)
            .ok_or_else(|| anyhow!("node not found"))?;
        let blocking = node.blocking_wait_gates(Utc::now());
        if !blocking.is_empty() {
            let gates: Vec<serde_json::Value> = blocking
                .iter()
                .map(|g| {
                    json!({
                        "gate_id": g.gate_id,
                        "kind": g.kind,
                        "description": g.description,
                        "outcome": g.outcome
                    })
                })
                .collect();
            return Err(GateError::new(
                gate_ids::PICKUP,
                "wait_gate_unresolved",
                format!(
                    "node {} has {} unresolved wait-gate(s): {}",
                    node_id,
                    blocking.len(),
                    blocking
                        .iter()
                        .map(|g| format!("{} ({})", g.gate_id, g.kind))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .at(&dag.dag_id, Some(node_id))
            .with_details(json!({ "gates": gates }))
            .into());
        }
        let unmet: Vec<String> = dag
            .edges
            .iter()
            .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == node_id)
            .filter(|e| {
                dag.nodes
                    .get(&e.from_node_id)
                    .map(|n| n.status != "DONE")
                    .unwrap_or(true)
            })
            .map(|e| e.from_node_id.clone())
            .collect();
        if !unmet.is_empty() {
            return Err(GateError::new(
                gate_ids::PICKUP,
                "blocked_by_unmet",
                format!("node {} is blocked by {}", node_id, unmet.join(", ")),
            )
            .at(&dag.dag_id, Some(node_id))
            .with_details(json!({ "blocked_by": unmet }))
            .into());
        }
        if node.status != "READY" {
            return Err(GateError::new(
                gate_ids::PICKUP,
                "node_not_ready",
                format!("node {} is {}, not READY", node_id, node.status),
            )
            .at(&dag.dag_id, Some(node_id))
            .with_details(json!({ "status": node.status }))
            .into());
        }
        Ok(())
    }

    /// Resolve `{{ <node>.output }}` / `{{ <node>.output_path }}` in the
    /// node description. `Ok(None)` when the description has no placeholders.
    /// `{{ x.output }}` text is wrapped in `fence` (S7) and the fence
    /// instruction is prepended once when any output was inlined.
    fn resolve_node_description(
        &self,
        dag: &DagState,
        node_id: &str,
        fence: &Fence,
    ) -> Result<Option<String>> {
        let node = dag
            .nodes
            .get(node_id)
            .ok_or_else(|| anyhow!("node not found"))?;
        let Some(description) = node.description.as_deref() else {
            return Ok(None);
        };
        let refs = placeholders::node_refs(description);
        if refs.is_empty() {
            return Ok(None);
        }
        let preds = collect_blocked_by_predecessors(dag, node_id);
        let mut values: HashMap<(String, PlaceholderField), String> = HashMap::new();
        for r in &refs {
            if !preds.contains(&r.node_id) {
                return Err(GateError::new(
                    gate_ids::PICKUP,
                    "template_ref_not_predecessor",
                    format!(
                        "{} references {} which is not a blocked_by predecessor of {}",
                        r.raw, r.node_id, node_id
                    ),
                )
                .at(&dag.dag_id, Some(node_id))
                .with_details(json!({ "ref": r.raw, "ref_node_id": r.node_id }))
                .into());
            }
            let Some(output) = dag.nodes.get(&r.node_id).and_then(|n| n.output.as_ref()) else {
                return Err(GateError::new(
                    gate_ids::PICKUP,
                    "template_ref_output_missing",
                    format!("{} has no recorded output for {}", r.node_id, r.raw),
                )
                .at(&dag.dag_id, Some(node_id))
                .with_details(json!({ "ref": r.raw, "ref_node_id": r.node_id }))
                .into());
            };
            let key = (r.node_id.clone(), r.field);
            if values.contains_key(&key) {
                continue;
            }
            let text = self.read_node_output(output)?;
            let value = match r.field {
                PlaceholderField::Output => fence.wrap(&format!("node:{}", r.node_id), &text),
                PlaceholderField::OutputPath => {
                    let path = self.ensure_output_view(output, &text)?;
                    path.to_string_lossy().to_string()
                }
            };
            values.insert(key, value);
        }
        let rendered = placeholders::render_node_refs(description, |r| {
            values.get(&(r.node_id.clone(), r.field)).cloned()
        });
        if refs.iter().any(|r| r.field == PlaceholderField::Output) {
            Ok(Some(format!("{}\n\n{}", fence.instruction(), rendered)))
        } else {
            Ok(Some(rendered))
        }
    }

    /// Read a recorded node output from its immutable blob.
    fn read_node_output(&self, output: &crate::work::NodeOutputRef) -> Result<String> {
        let path = self.receipts.blob_path(&output.blob_id);
        std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("node output blob {} unreadable: {}", output.blob_id, e))
    }

    /// The derived `nodes/<node_id>.out.md` view; rebuilt from the blob when
    /// missing or stale. Returns the absolute path.
    fn ensure_output_view(
        &self,
        output: &crate::work::NodeOutputRef,
        text: &str,
    ) -> Result<PathBuf> {
        let path = self.root_dir.join(&output.output_path);
        let current = std::fs::read_to_string(&path).ok();
        if current.as_deref() != Some(text) {
            if let Some(parent) = path.parent() {
                ensure_dir(parent)?;
            }
            std::fs::write(&path, text)?;
        }
        Ok(path)
    }

    pub async fn wih_sign_open(&self, wih_id: &str, signature: &str) -> Result<()> {
        let scope = EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "WIHOpenSigned".to_string(),
            payload: json!({ "wih_id": wih_id, "signature": signature }),
            provenance: None,
        };
        self.emit(evt).await?;
        self.refresh_wih_view(wih_id).await?;
        Ok(())
    }

    pub async fn turn_closeout(&self, wih_id: &str) -> Result<()> {
        let scope = EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        let events = self.events_for_wih(wih_id).await?;
        let receipt_ids: Vec<String> = events
            .iter()
            .filter(|evt| evt.r#type == "ReceiptWritten")
            .filter_map(|evt| {
                evt.payload
                    .get("receipt_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        let receipts_count = receipt_ids.len();
        let active_paths = self.leases.active_paths_for_wih(wih_id).await?;
        let released = self.leases.release_for_wih(wih_id).await?;
        let heartbeat = Utc::now().to_rfc3339();
        let event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "GateTurnCloseout".to_string(),
            payload: json!({
                "wih_id": wih_id,
                "receipts": receipts_count,
                "receipt_ids": receipt_ids,
                "leases_released": released.len(),
                "released_ids": released,
                "active_paths": active_paths,
                "last_heartbeat": heartbeat
            }),
            provenance: None,
        };
        self.emit(event).await?;
        self.refresh_wih_view(wih_id).await?;
        Ok(())
    }

    /// Gate 2 base checks (open-signed, allowed tools, lease coverage). The
    /// public `pre_tool` / `pre_tool_with` (gate_judge.rs) add the optional
    /// judge step on top.
    async fn pre_tool_base(
        &self,
        wih_id: &str,
        tool: &str,
        paths_touched: &[String],
    ) -> Result<GateResult> {
        let scope = EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        let wih_events = self.events_for_wih(wih_id).await?;
        let wih = project_wih(&wih_events, wih_id).ok_or_else(|| anyhow!("wih not found"))?;
        if !wih.open_signed {
            return Ok(GateResult {
                allowed: false,
                reason: Some("WIHOpenSigned required".to_string()),
            });
        }

        if let Some(allowed) = self.allowed_tools_for_wih(wih_id, &wih_events) {
            if !allowed.contains(&tool.to_string()) {
                return Ok(GateResult {
                    allowed: false,
                    reason: Some("tool not allowed by WIH policy".to_string()),
                });
            }
        }

        if !paths_touched.is_empty() {
            let now = Utc::now().to_rfc3339();
            let (ok, missing) = self.leases.check_coverage(paths_touched, &now).await?;
            if !ok {
                return Ok(GateResult {
                    allowed: false,
                    reason: Some(format!("missing lease coverage: {:?}", missing)),
                });
            }
        }

        Ok(GateResult {
            allowed: true,
            reason: Some(format!("allowed tool {}", tool)),
        })
    }

    pub async fn post_tool(
        &self,
        wih_id: &str,
        tool: &str,
        receipt_payload: serde_json::Value,
    ) -> Result<String> {
        self.post_tool_with_usage(wih_id, tool, receipt_payload, None, None)
            .await
    }

    pub async fn post_tool_with_usage(
        &self,
        wih_id: &str,
        tool: &str,
        receipt_payload: serde_json::Value,
        mut input_tokens: Option<i32>,
        mut output_tokens: Option<i32>,
    ) -> Result<String> {
        // Automatic estimation if missing
        if input_tokens.is_none() {
            input_tokens = Some(crate::core::telemetry::estimate_payload_tokens(&receipt_payload));
        }
        if output_tokens.is_none() {
            output_tokens = Some(10); // Minimum output estimate
        }

        let receipt_id = create_receipt_id();
        let total_tokens = match (input_tokens, output_tokens) {
            (Some(i), Some(o)) => Some(i + o),
            (Some(i), None) => Some(i),
            (None, Some(o)) => Some(o),
            (None, None) => None,
        };

        let receipt = ReceiptRecord {
            receipt_id: receipt_id.clone(),
            run_id: format!("run_{}", wih_id),
            step: None,
            tool: tool.to_string(),
            tool_version: None,
            inputs_ref: None,
            outputs_ref: None,
            exit: None,
            input_tokens,
            output_tokens,
            total_tokens,
        };
        // Side-effecting tool calls are recorded on the run's signed chain and
        // deduped by idempotency key; read-only calls keep the legacy path.
        let run_id = receipt.run_id.clone();
        let receipt_id = self.receipts.record_tool_effect(&run_id, tool, &receipt_payload, || {
            self.receipts.write_receipt(&receipt)?;
            Ok(receipt_id.clone())
        })?;

        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "ReceiptWritten".to_string(),
            payload: json!({
                "wih_id": wih_id,
                "receipt_id": receipt_id,
                "tool": tool,
                "payload": receipt_payload,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "total_tokens": total_tokens
                }
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        self.turn_closeout(wih_id).await?;
        self.refresh_wih_view(wih_id).await?;
        Ok(receipt_id)
    }

    pub async fn rollback_wih(&self, wih_id: &str) -> Result<()> {
        let backups_dir = self.root_dir.join(".allternit").join("backups");
        if !backups_dir.exists() {
            return Err(anyhow!("No backups directory found"));
        }

        // Find the latest backup for this WIH
        let mut latest_backup = None;
        let mut latest_ts = 0;

        if let Ok(entries) = std::fs::read_dir(&backups_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(wih_id) {
                    let parts: Vec<&str> = name.split('_').collect();
                    if let Some(ts_str) = parts.last() {
                        if let Ok(ts) = ts_str.parse::<i64>() {
                            if ts > latest_ts {
                                latest_ts = ts;
                                latest_backup = Some(entry.path());
                            }
                        }
                    }
                }
            }
        }

        let backup_path = latest_backup.ok_or_else(|| anyhow!("No backup found for WIH {}", wih_id))?;
        tracing::info!("Rolling back WIH {} using backup: {:?}", wih_id, backup_path);

        // Restore files from backup to root
        let mut stack = vec![backup_path.clone()];
        while let Some(current_dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&current_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.is_file() {
                        if let Ok(rel_path) = path.strip_prefix(&backup_path) {
                            let target_path = self.root_dir.join(rel_path);
                            if let Some(parent) = target_path.parent() {
                                crate::core::io::ensure_dir(parent)?;
                            }
                            std::fs::copy(&path, &target_path)?;
                        }
                    }
                }
            }
        }

        let rolled_back = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "GateAutolandRolledBack".to_string(),
            payload: json!({ "wih_id": wih_id, "backup_used": backup_path.to_string_lossy() }),
            provenance: None,
        };
        self.emit(rolled_back).await?;

        Ok(())
    }

    pub async fn autoland_wih(&self, wih_id: &str, dry_run: bool, git_commit: bool) -> Result<AutolandResult> {
        let scope = EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;

        let wih_events = self.events_for_wih(wih_id).await?;
        let _wih_state = project_wih(&wih_events, wih_id).ok_or_else(|| anyhow!("wih not found"))?;

        let mut has_pass = false;
        for evt in &wih_events {
            if evt.r#type == "WIHClosedSigned" {
                if evt.payload.get("final_status").and_then(|s| s.as_str()) == Some("PASS") {
                    has_pass = true;
                }
            }
        }
        if !has_pass {
            return Err(anyhow!("WIH must be closed with PASS status to autoland"));
        }

        // 2. Visual verification check (if enabled)
        if let (Some(visual_provider), Some(visual_config)) = (&self.visual_provider, &self.visual_config) {
            if visual_config.enabled {
                let evidence = visual_provider
                    .gather_evidence(wih_id)
                    .await
                    .map_err(|e: ProviderError| anyhow!("Visual verification failed: {}", e))?;

                if !evidence.success || evidence.overall_confidence < visual_config.min_confidence {
                    return Err(anyhow!(
                        "Visual confidence {:.1}% below threshold {:.1}%",
                        evidence.overall_confidence * 100.0,
                        visual_config.min_confidence * 100.0
                    ));
                }

                // 3. Log verification success
                tracing::info!(
                    "Visual verification passed for {}: {:.1}% confidence",
                    wih_id,
                    evidence.overall_confidence * 100.0
                );

                // Emit visual verification event for audit trail
                let verified_event = AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: Some(scope.clone()),
                    r#type: "GateVisualVerified".to_string(),
                    payload: json!({
                        "wih_id": wih_id,
                        "confidence": evidence.overall_confidence,
                        "artifact_count": evidence.artifacts.len(),
                        "threshold": visual_config.min_confidence,
                        "provider": format!("{:?}", visual_config.provider_type),
                    }),
                    provenance: None,
                };
                self.emit(verified_event).await?;
            }
        }

        let mut impact = AutolandImpact {
            modified: Vec::new(),
            added: Vec::new(),
            deleted: Vec::new(),
        };

        let runner_dir = self.root_dir.join(".allternit").join("runner").join(wih_id);
        let mut backup_path = None;

        if runner_dir.exists() && runner_dir.is_dir() {
            // First pass: Calculate impact and prepare for rollback
            let mut files_to_copy = Vec::new();
            let mut stack = vec![runner_dir.clone()];
            while let Some(current_dir) = stack.pop() {
                if let Ok(entries) = std::fs::read_dir(&current_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            stack.push(path);
                        } else if path.is_file() {
                            if let Ok(rel_path) = path.strip_prefix(&runner_dir) {
                                let target_path = self.root_dir.join(rel_path);
                                let rel_path_str = rel_path.to_string_lossy().to_string();
                                if target_path.exists() {
                                    impact.modified.push(rel_path_str);
                                } else {
                                    impact.added.push(rel_path_str);
                                }
                                files_to_copy.push((path, target_path));
                            }
                        }
                    }
                }
            }

            if !dry_run {
                // Prepare backup for rollback
                let backup_dir = self.root_dir.join(".allternit").join("backups").join(format!("{}_{}", wih_id, Utc::now().timestamp()));
                crate::core::io::ensure_dir(&backup_dir)?;
                backup_path = Some(backup_dir.to_string_lossy().to_string());

                for (_, target_path) in &files_to_copy {
                    if target_path.exists() {
                        if let Ok(rel) = target_path.strip_prefix(&self.root_dir) {
                            let b_path = backup_dir.join(rel);
                            if let Some(p) = b_path.parent() {
                                crate::core::io::ensure_dir(p)?;
                            }
                            std::fs::copy(target_path, b_path)?;
                        }
                    }
                }

                // Execute copies with rollback on failure
                for (source, target) in files_to_copy {
                    if let Some(parent) = target.parent() {
                        if let Err(e) = crate::core::io::ensure_dir(parent) {
                            tracing::error!("Autoland rollback triggered: {}", e);
                            return Err(anyhow!("Autoland failed during directory creation: {}", e));
                        }
                    }
                    if let Err(e) = std::fs::copy(&source, &target) {
                        tracing::error!("Autoland rollback triggered: {}", e);
                        return Err(anyhow!("Autoland failed during file copy: {}", e));
                    }
                }

                let landed = AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "GateAutolanded".to_string(),
                    payload: json!({ "wih_id": wih_id, "impact": impact, "git_committed": git_commit }),
                    provenance: None,
                };
                self.emit(landed).await?;

                // 🤖 A2A Synchronization
                let sync_evt = AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeSynchronized".to_string(),
                    payload: json!({ "wih_id": wih_id, "status": "COMPLETED" }),
                    provenance: None,
                };
                self.emit(sync_evt).await?;

                // 🧹 Cleanup
                if let Some(vault) = &self.vault {
                    let _ = vault.archive_wih(wih_id).await;
                }

                // 🌲 Git Integration: Optional Atomic Commit
                if git_commit {
                    let git_dir = self.root_dir.join(".git");
                    if git_dir.exists() {
                        tracing::info!("Git detected and commit requested. Committing changes for {}", wih_id);
                        let _ = std::process::Command::new("git")
                            .args(&["add", "."])
                            .current_dir(&self.root_dir)
                            .output();
                        
                        let commit_msg = format!("autoland: implementation run {} landed\n\nProof of Work: .allternit/autoland/{}.jsonl", wih_id, wih_id);
                        let _ = std::process::Command::new("git")
                            .args(&["commit", "-m", &commit_msg])
                            .current_dir(&self.root_dir)
                            .output();
                    }
                }
            }
        }

        self.refresh_wih_view(wih_id).await?;

        Ok(AutolandResult {
            success: !dry_run,
            dry_run,
            impact,
            backup_dir: backup_path,
        })
    }

    pub async fn wih_close(
        &self,
        wih_id: &str,
        status: &str,
        evidence_refs: &[String],
    ) -> Result<()> {
        self.wih_close_with(wih_id, status, evidence_refs, None)
            .await
            .map(|_| ())
    }

    /// Gate 4 close. With `output`, the text is stored as an immutable blob
    /// behind a `node.output` receipt (`ReceiptWritten`), the derived view
    /// `.allternit/work/dags/<dag_id>/nodes/<node_id>.out.md` is written, and
    /// `DagNodeOutputRecorded` is emitted before the close events. The output
    /// receipt counts as evidence (`receipt:<id>` is appended to the refs).
    /// Returns the output receipt id when an output was recorded.
    pub async fn wih_close_with(
        &self,
        wih_id: &str,
        status: &str,
        evidence_refs: &[String],
        output: Option<&str>,
    ) -> Result<Option<String>> {
        self.wih_close_as(wih_id, status, evidence_refs, output, None)
            .await
            .map(|c| c.output_receipt_id)
    }

    /// Gate 4 close with an explicit closer. With the node/plan judge policy
    /// off (the default) this is exactly `wih_close_with`. `close_by:
    /// verifier` refuses a DONE/PASS close by the worker (`closer` None, the
    /// gate, or the WIH's own agent); `verify: judge` asks the judge for a
    /// verdict and lands the node DONE / EXCEPTION / NEEDS_HUMAN
    /// (`gate_judge.rs`, `spec/JUDGE.md`).
    pub async fn wih_close_as(
        &self,
        wih_id: &str,
        status: &str,
        evidence_refs: &[String],
        output: Option<&str>,
        closer: Option<&Actor>,
    ) -> Result<CloseOutcome> {
        let scope = EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        if evidence_refs.is_empty() && output.is_none() {
            return Err(anyhow!("evidence required to close WIH"));
        }
        let wih_events = self.events_for_wih(wih_id).await?;
        let wih_state = project_wih(&wih_events, wih_id).ok_or_else(|| anyhow!("wih not found"))?;
        let dag_id = wih_state.dag_id.clone();
        let node_id = wih_state.node_id.clone();
        let judge_policy = self.gate4_precheck(&wih_state, status, closer).await?;
        let requested_status = status;

        let mut evidence_refs: Vec<String> = evidence_refs.to_vec();
        let output_receipt_id = match output {
            Some(text) => {
                let receipt_id = self
                    .record_node_output(wih_id, &dag_id, &node_id, text)
                    .await?;
                evidence_refs.push(format!("receipt:{receipt_id}"));
                Some(receipt_id)
            }
            None => None,
        };
        let evidence_refs = evidence_refs.as_slice();
        let decision = self
            .gate4_verdict(&wih_state, requested_status, closer, &judge_policy, evidence_refs, output)
            .await?;

        let close_req = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "WIHCloseRequested".to_string(),
            payload: json!({ "wih_id": wih_id, "dag_id": dag_id, "node_id": node_id, "status": requested_status, "evidence_refs": evidence_refs }),
            provenance: None,
        };
        self.emit(close_req).await?;
        // From here on `status` is the verdict-mapped status (unchanged when
        // the judge policy is off).
        let status = decision.final_status.as_str();

        let closed = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "WIHClosedSigned".to_string(),
            payload: json!({ "wih_id": wih_id, "dag_id": dag_id, "node_id": node_id, "final_status": status, "closed_at": Utc::now().to_rfc3339() }),
            provenance: None,
        };
        self.emit(closed).await?;

        let status_evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagNodeStatusChanged".to_string(),
            payload: json!({ "dag_id": dag_id, "node_id": node_id, "to": status }),
            provenance: None,
        };
        self.emit(status_evt).await?;

        let archived = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "WIHArchived".to_string(),
            payload: json!({ "wih_id": wih_id, "dag_id": dag_id, "node_id": node_id }),
            provenance: None,
        };
        self.emit(archived).await?;

        let vault_job = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "VaultJobCreated".to_string(),
            payload: json!({ "wih_id": wih_id }),
            provenance: None,
        };
        self.emit(vault_job).await?;

        if let Some(vault) = &self.vault {
            let _ = vault.archive_wih(wih_id).await;
        }

        // 🚀 Autonomous Loop Trigger: If status is PASS and autoland_on_pass is set, land now!
        if status == "PASS" {
            if let Some(policy) = wih_state.loop_policy {
                if policy.autoland_on_pass {
                    tracing::info!("Autonomous Loop: Triggering Autoland for WIH {}", wih_id);
                    // Default git_commit to FALSE for safety in autonomous loop
                    if let Err(e) = self.autoland_wih(wih_id, false, false).await {
                        tracing::error!("Autonomous Loop: Autoland failed for {}: {}", wih_id, e);
                    }
                }
            }
        }

        self.refresh_wih_view(wih_id).await?;
        self.refresh_dag_view(&dag_id).await?;

        Ok(CloseOutcome {
            output_receipt_id,
            final_status: decision.final_status.clone(),
            node_status: decision.node_status.clone(),
            verdict: decision.verdict.clone(),
        })
    }

    pub async fn lease_request(
        &self,
        wih_id: &str,
        agent_id: &str,
        paths: Vec<String>,
        ttl_seconds: Option<i64>,
    ) -> Result<String> {
        let lease_id = create_lease_id();
        let req = LeaseRequest {
            lease_id: lease_id.clone(),
            wih_id: wih_id.to_string(),
            agent_id: agent_id.to_string(),
            paths,
            requested_at: Utc::now().to_rfc3339(),
            ttl_seconds,
        };
        self.leases.request(req).await?;
        Ok(lease_id)
    }

    // --- MCP Agent Ported Features ---

    async fn check_idempotency(&self, id_field: &str, id_value: &str) -> Result<bool> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        for evt in events {
            if let Some(val) = evt.payload.get(id_field).and_then(|v| v.as_str()) {
                if val == id_value {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub async fn elicitation_request(
        &self,
        wih_id: &str,
        message: &str,
        requested_schema: Option<serde_json::Value>,
        url: Option<&str>,
        server_name: Option<&str>,
        timeout_seconds: Option<i32>,
    ) -> Result<String> {
        let elicitation_id = format!("el_{}", rand::random::<u32>() % 1_000_000);
        
        let req = crate::core::types::ElicitationRequest {
            elicitation_id: elicitation_id.clone(),
            message: message.to_string(),
            requested_schema: requested_schema.clone(),
            url: url.map(|s| s.to_string()),
            server_name: server_name.map(|s| s.to_string()),
            timeout_seconds,
        };

        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: Some(EventScope {
                wih_id: Some(wih_id.to_string()),
                ..Default::default()
            }),
            r#type: "ElicitationRequested".to_string(),
            payload: serde_json::to_value(&req)?,
            provenance: None,
        };
        self.emit(evt).await?;
        
        // Write projection for UI
        let _ = write_elicitation_view(&self.root_dir, &req)?;
        
        Ok(elicitation_id)
    }

    pub async fn elicitation_response(
        &self,
        wih_id: &str,
        elicitation_id: &str,
        status: &str, // "approved", "declined", "cancelled"
        response: Option<serde_json::Value>,
        reason: Option<&str>,
    ) -> Result<()> {
        if self.check_idempotency("elicitation_id", elicitation_id).await? {
            // Already responded or exists
            // Logic could be more sophisticated here, but for now we just log
        }

        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: Some(EventScope {
                wih_id: Some(wih_id.to_string()),
                ..Default::default()
            }),
            r#type: "ElicitationResponded".to_string(),
            payload: json!({
                "elicitation_id": elicitation_id,
                "status": status,
                "response": response,
                "reason": reason
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        
        // Clean up projection
        let proj_path = self.root_dir.join(".allternit/ledger/elicitation/active").join(format!("{}.json", elicitation_id));
        if proj_path.exists() {
            let _ = std::fs::remove_file(proj_path);
        }

        Ok(())
    }

    pub async fn agent_handoff_request(
        &self,
        wih_id: &str,
        source_agent_id: &str,
        target_agent_id: &str,
        reason: Option<&str>,
        context_variables: Option<serde_json::Value>,
    ) -> Result<()> {
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: Some(EventScope {
                wih_id: Some(wih_id.to_string()),
                ..Default::default()
            }),
            r#type: "AgentHandoffRequested".to_string(),
            payload: json!({
                "source_agent_id": source_agent_id,
                "target_agent_id": target_agent_id,
                "reason": reason,
                "context_variables": context_variables
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        
        // Update WIH projection automatically
        let wih_events = self.events_for_wih(wih_id).await?;
        if let Some(mut wih) = project_wih(&wih_events, wih_id) {
            wih.agent_id = Some(target_agent_id.to_string());
            let _ = write_wih_view(&self.root_dir, &wih.dag_id, &wih)?;
        }

        Ok(())
    }

    pub async fn sampling_request(
        &self,
        wih_id: &str,
        messages: Vec<serde_json::Value>,
        system_prompt: Option<&str>,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        stop_sequences: Option<Vec<String>>,
    ) -> Result<String> {
        let sampling_id = format!("sam_{}", rand::random::<u32>() % 1_000_000);
        
        let req = crate::core::types::SamplingRequest {
            sampling_id: sampling_id.clone(),
            messages,
            system_prompt: system_prompt.map(|s| s.to_string()),
            max_tokens,
            temperature,
            stop_sequences,
        };

        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: Some(EventScope {
                wih_id: Some(wih_id.to_string()),
                ..Default::default()
            }),
            r#type: "SamplingRequested".to_string(),
            payload: serde_json::to_value(&req)?,
            provenance: None,
        };
        self.emit(evt).await?;
        
        // Write projection
        let _ = write_sampling_view(&self.root_dir, &req)?;

        Ok(sampling_id)
    }

    pub async fn sampling_response(
        &self,
        wih_id: &str,
        sampling_id: &str,
        role: &str,
        content: serde_json::Value,
        model: &str,
    ) -> Result<()> {
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: Some(EventScope {
                wih_id: Some(wih_id.to_string()),
                ..Default::default()
            }),
            r#type: "SamplingResponded".to_string(),
            payload: json!({
                "sampling_id": sampling_id,
                "role": role,
                "content": content,
                "model": model
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        
        // Clean up projection
        let proj_path = self.root_dir.join(".allternit/ledger/sampling/active").join(format!("{}.json", sampling_id));
        if proj_path.exists() {
            let _ = std::fs::remove_file(proj_path);
        }

        Ok(())
    }

    pub async fn register_session_proxy(
        &self,
        session_id: &str,
        upstream_actor: crate::core::types::Actor,
        targets: Vec<String>,
    ) -> Result<()> {
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "SessionProxyRegistered".to_string(),
            payload: json!({
                "session_id": session_id,
                "upstream_actor": upstream_actor,
                "targets": targets,
                "created_at": Utc::now().to_rfc3339()
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        Ok(())
    }

    pub async fn store_oauth_token(&self, token: crate::core::types::OAuthTokenRecord) -> Result<()> {
        if let Some(v) = &self.oauth_vault {
            v.store_token(&token)?;
            
            let evt = AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: gate_actor(&self.actor_id),
                scope: None,
                r#type: "OAuthTokenStored".to_string(),
                payload: json!({
                    "server_name": token.server_name,
                    "token_type": token.token_type,
                    "scope": token.scope,
                    "expires_at": token.expires_at
                }),
                provenance: None,
            };
            self.emit(evt).await?;
        }
        Ok(())
    }

    pub async fn get_oauth_token(&self, server_name: &str) -> Result<Option<crate::core::types::OAuthTokenRecord>> {
        if let Some(v) = &self.oauth_vault {
            return v.get_token(server_name);
        }
        Ok(None)
    }

    async fn events_for_dag(&self, dag_id: &str) -> Result<Vec<AllternitEvent>> {
        let all = self.ledger.query(LedgerQuery::default()).await?;
        Ok(all
            .into_iter()
            .filter(|evt| match evt.payload.get("dag_id") {
                Some(v) => v.as_str() == Some(dag_id),
                None => false,
            })
            .collect())
    }

    async fn events_for_wih(&self, wih_id: &str) -> Result<Vec<AllternitEvent>> {
        let all = self.ledger.query(LedgerQuery::default()).await?;
        Ok(all
            .into_iter()
            .filter(|evt| match evt.payload.get("wih_id") {
                Some(v) => v.as_str() == Some(wih_id),
                None => false,
            })
            .collect())
    }

    async fn emit(&self, event: AllternitEvent) -> Result<()> {
        self.ledger.append(event.clone()).await?;
        // Timer wait-gates register a keyed wake so a sweep flips readiness.
        if let Some(wake) = crate::wake::timer_gate_wake_event(&event) {
            self.ledger.append(wake).await?;
        }
        if let Some(index) = &self.index {
            let _ = index.index_event(&event).await;
        }
        Ok(())
    }

    async fn refresh_dag_view(&self, dag_id: &str) -> Result<()> {
        let events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&events, dag_id);
        let _ = write_dag_view(&self.root_dir, &dag)?;
        Ok(())
    }

    async fn refresh_wih_view(&self, wih_id: &str) -> Result<()> {
        let events = self.events_for_wih(wih_id).await?;
        if let Some(wih) = project_wih(&events, wih_id) {
            let _ = write_wih_view(&self.root_dir, &wih.dag_id, &wih)?;
        }
        Ok(())
    }

    async fn refresh_prompt_view(&self, prompt_id: &str) -> Result<()> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        if let Some(prompt) = project_prompt(&events, prompt_id) {
            let _ = write_prompt_view(&self.root_dir, &prompt)?;
        }
        Ok(())
    }

    async fn ensure_policy_scope(&self, scope: &EventScope) -> Result<()> {
        policy::ensure_injected(&self.root_dir, &self.ledger, Some(scope.clone()), "gate")
            .await
            .map_err(|err| anyhow!("policy injection required for scope {:?}: {}", scope, err))
    }

    pub async fn record_agent_decision(
        &self,
        note: &str,
        reason: Option<String>,
        linked_event_ids: Vec<String>,
    ) -> Result<String> {
        if self.strict_provenance && linked_event_ids.is_empty() {
            return Err(anyhow!(
                "agent decision requires linked mutations in strict mode"
            ));
        }
        let decision_id = format!("dec_{}", rand::random::<u32>() % 1_000_000);
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "AgentDecisionRecorded".to_string(),
            payload: json!({
                "decision_id": decision_id,
                "note": note,
                "reason": reason,
                "linked_event_ids": linked_event_ids
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        Ok(decision_id)
    }

    pub async fn mutate_with_decision(
        &self,
        dag_id: &str,
        note: &str,
        reason: Option<String>,
        mutations: Vec<DagMutation>,
    ) -> Result<(String, Vec<String>)> {
        let scope = EventScope {
            dag_id: Some(dag_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        if mutations.is_empty() {
            return Err(anyhow!("mutations required"));
        }
        self.validate_mutations(dag_id, &mutations, None, None).await?;
        let mut events = Vec::new();
        for mutation in mutations {
            let event = match mutation {
                DagMutation::CreateNode {
                    node_id,
                    node_kind,
                    title,
                    parent_node_id,
                    execution_mode,
                    description,
                    executor,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeCreated".to_string(),
                    payload: json!({
                        "node_id": node_id,
                        "dag_id": dag_id,
                        "node_kind": node_kind,
                        "title": title,
                        "parent_node_id": parent_node_id,
                        "execution_mode": execution_mode,
                        "description": description,
                        "executor": executor
                    }),
                    provenance: None,
                },
                DagMutation::UpdateNode { node_id, patch } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeUpdated".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "patch": patch
                    }),
                    provenance: None,
                },
                DagMutation::DeleteNode { node_id } => {
                    // Project from the full event log: DagNodeUpdated events carry no
                    // dag_id in their payload, so the dag-scoped event filter would
                    // miss renames and serve a stale title.
                    let all_events = self.ledger.query(LedgerQuery::default()).await?;
                    let dag = project_dag(&all_events, dag_id);
                    let (title, parent_node_id) = dag
                        .nodes
                        .get(&node_id)
                        .map(|n| (Some(n.title.clone()), n.parent_node_id.clone()))
                        .unwrap_or((None, None));
                    AllternitEvent {
                        event_id: create_event_id(),
                        ts: Utc::now().to_rfc3339(),
                        actor: gate_actor(&self.actor_id),
                        scope: None,
                        r#type: "DagNodeRemoved".to_string(),
                        payload: json!({
                            "dag_id": dag_id,
                            "node_id": node_id,
                            "title": title,
                            "parent_node_id": parent_node_id
                        }),
                        provenance: None,
                    }
                }
                DagMutation::ReparentNode {
                    node_id,
                    new_parent_id,
                } => {
                    // Project from the full event log: the dag-scoped event
                    // filter misses dag_id-less events (e.g. pre-v5
                    // WIHPickedUp), so a dag projected from filtered events
                    // can serve a stale parent chain to the cycle check.
                    let all_events = self.ledger.query(LedgerQuery::default()).await?;
                    let dag = project_dag(&all_events, dag_id);
                    let old_parent_id = dag
                        .nodes
                        .get(&node_id)
                        .ok_or_else(|| anyhow!("node not found: {}", node_id))?
                        .parent_node_id
                        .clone();
                    if let Some(parent) = &new_parent_id {
                        if !dag.nodes.contains_key(parent) {
                            return Err(anyhow!("new parent not found in dag: {}", parent));
                        }
                        if would_create_parent_cycle(&dag, &node_id, parent) {
                            return Err(anyhow!(
                                "reparent_node would create parent cycle: {} under {}",
                                node_id,
                                parent
                            ));
                        }
                    }
                    AllternitEvent {
                        event_id: create_event_id(),
                        ts: Utc::now().to_rfc3339(),
                        actor: gate_actor(&self.actor_id),
                        scope: None,
                        r#type: "DagNodeReparented".to_string(),
                        payload: json!({
                            "dag_id": dag_id,
                            "node_id": node_id,
                            "new_parent_id": new_parent_id,
                            "old_parent_id": old_parent_id
                        }),
                        provenance: None,
                    }
                }
                DagMutation::AddBlockedBy {
                    from_node_id,
                    to_node_id,
                } => {
                    self.ensure_no_cycle(dag_id, &from_node_id, &to_node_id)
                        .await?;
                    AllternitEvent {
                        event_id: create_event_id(),
                        ts: Utc::now().to_rfc3339(),
                        actor: gate_actor(&self.actor_id),
                        scope: None,
                        r#type: "DagEdgeAdded".to_string(),
                        payload: json!({
                            "dag_id": dag_id,
                            "from_node_id": from_node_id,
                            "to_node_id": to_node_id,
                            "edge_type": "blocked_by"
                        }),
                        provenance: None,
                    }
                }
                DagMutation::AddRelation {
                    a,
                    b,
                    note,
                    context_share,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagRelationAdded".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "a": a,
                        "b": b,
                        "note": note,
                        "context_share": context_share.unwrap_or(false)
                    }),
                    provenance: None,
                },
                DagMutation::AddLabel { node_id, label } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "LabelAdded".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "label": label
                    }),
                    provenance: None,
                },
                DagMutation::RemoveLabel { node_id, label } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "LabelRemoved".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "label": label
                    }),
                    provenance: None,
                },
                DagMutation::SetState {
                    node_id,
                    dimension,
                    value,
                    reason,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "StateSet".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "dimension": dimension,
                        "value": value,
                        "reason": reason
                    }),
                    provenance: None,
                },
                DagMutation::AddComment {
                    node_id,
                    body_ref,
                    author,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "CommentAdded".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "comment_id": create_event_id(),
                        "author": author,
                        "body_ref": body_ref
                    }),
                    provenance: None,
                },
                DagMutation::AddWaitGate {
                    node_id,
                    gate_id,
                    kind,
                    description,
                    params,
                } => self.wait_gate_added_event(
                    dag_id,
                    &node_id,
                    gate_id,
                    kind,
                    description,
                    params,
                    None,
                ),
                DagMutation::ChangeStatus {
                    node_id,
                    from,
                    to,
                    reason,
                } => AllternitEvent {
                    event_id: create_event_id(),
                    ts: Utc::now().to_rfc3339(),
                    actor: gate_actor(&self.actor_id),
                    scope: None,
                    r#type: "DagNodeStatusChanged".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "from": from,
                        "to": to,
                        "reason": reason
                    }),
                    provenance: None,
                },
            };
            if self.strict_provenance && event.r#type == "DagRelationAdded" {
                // Enforce that related_to context_share is explicit in strict mode
                let explicit = event
                    .payload
                    .get("context_share")
                    .and_then(|v| v.as_bool())
                    .is_some();
                if !explicit {
                    return Err(anyhow!("context_share must be explicit in strict mode"));
                }
            }
            events.push(event);
        }

        let mutation_ids: Vec<String> = events.iter().map(|e| e.event_id.clone()).collect();
        let decision_id = self
            .record_agent_decision(note, reason, mutation_ids.clone())
            .await?;
        let provenance = MutationProvenance {
            prompt_id: None,
            delta_id: None,
            agent_decision_id: Some(decision_id.clone()),
        };
        for mut evt in events {
            evt.provenance = Some(self.provenance_from(&provenance));
            if let Some(decision_id) = &provenance.agent_decision_id {
                if self.strict_provenance {
                    self.ensure_agent_decision_links(decision_id, &evt.event_id)
                        .await?;
                }
            }
            self.emit(evt).await?;
        }

        self.refresh_dag_view(dag_id).await?;
        Ok((decision_id, mutation_ids))
    }

    /// Gate 0 checks that need the whole mutation batch, run before anything
    /// is emitted: output placeholders in node descriptions must name a node
    /// that exists in the dag or is created in the same batch; executors must
    /// be `bot:<slug>` / `ao:<harness>`; wait-gates must target a live node
    /// with valid params. Rejections carry the prompt/delta provenance the
    /// batch was submitted under, and leave the ledger untouched.
    async fn validate_mutations(
        &self,
        dag_id: &str,
        mutations: &[DagMutation],
        prompt_id: Option<&str>,
        delta_id: Option<&str>,
    ) -> Result<()> {
        let dag_events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&dag_events, dag_id);
        let mut known: std::collections::HashSet<String> = dag.nodes.keys().cloned().collect();
        for m in mutations {
            if let DagMutation::CreateNode { node_id, .. } = m {
                known.insert(node_id.clone());
            }
        }
        let provenance = json!({ "prompt_id": prompt_id, "delta_id": delta_id });
        let reject = |code: &str, node_id: &str, reason: String, extra: serde_json::Value| {
            let mut details = json!({ "provenance": provenance });
            if let (Some(obj), Some(more)) = (details.as_object_mut(), extra.as_object()) {
                for (k, v) in more {
                    obj.insert(k.clone(), v.clone());
                }
            }
            Err(anyhow::Error::from(
                GateError::new(gate_ids::PLAN, code, reason)
                    .at(dag_id, Some(node_id))
                    .with_details(details),
            ))
        };
        for m in mutations {
            let (node_id, description, executor) = match m {
                DagMutation::CreateNode {
                    node_id,
                    description,
                    executor,
                    ..
                } => (node_id, description.clone(), executor.clone()),
                DagMutation::UpdateNode { node_id, patch } => (
                    node_id,
                    patch
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    patch
                        .get("executor")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ),
                DagMutation::AddWaitGate {
                    node_id,
                    kind,
                    params,
                    ..
                } => {
                    if !known.contains(node_id) {
                        return reject(
                            "wait_gate_unknown_node",
                            node_id,
                            format!("wait-gate targets unknown node {node_id}"),
                            json!({}),
                        );
                    }
                    if let Some(node) = dag.nodes.get(node_id) {
                        if node.status == "DONE" || node.status == "FAILED" {
                            return reject(
                                "wait_gate_terminal_node",
                                node_id,
                                format!("node {node_id} is already {}", node.status),
                                json!({ "status": node.status }),
                            );
                        }
                    }
                    if *kind == WaitGateKind::Timer {
                        let until_ok = params
                            .get("until")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<chrono::DateTime<Utc>>().ok())
                            .is_some();
                        if !until_ok {
                            return reject(
                                "wait_gate_invalid_params",
                                node_id,
                                "timer wait-gate requires params.until as an RFC 3339 timestamp"
                                    .to_string(),
                                json!({ "params": params }),
                            );
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            if let Some(executor) = executor.as_deref() {
                if let Err(reason) = crate::work::types::validate_executor(executor) {
                    return reject(
                        "invalid_executor",
                        node_id,
                        reason,
                        json!({ "executor": executor }),
                    );
                }
            }
            if let Some(description) = description.as_deref() {
                for r in placeholders::node_refs(description) {
                    if !known.contains(&r.node_id) {
                        return reject(
                            "template_ref_unknown_node",
                            node_id,
                            format!("{} references unknown node {}", r.raw, r.node_id),
                            json!({ "ref": r.raw, "ref_node_id": r.node_id }),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn wait_gate_added_event(
        &self,
        dag_id: &str,
        node_id: &str,
        gate_id: Option<String>,
        kind: WaitGateKind,
        description: Option<String>,
        params: HashMap<String, serde_json::Value>,
        provenance: Option<crate::core::types::EventProvenance>,
    ) -> AllternitEvent {
        let gate_id = gate_id.unwrap_or_else(mint_wait_gate_id);
        let description = description.unwrap_or_else(|| format!("{kind} gate"));
        AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagNodeWaitGateAdded".to_string(),
            payload: json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "gate_id": gate_id,
                "kind": kind,
                "description": description,
                "params": params
            }),
            provenance,
        }
    }

    /// Attach a wait-gate to a node through the normal Gate 0 refine path
    /// (prompt delta provenance). Returns the gate id.
    pub async fn add_node_wait_gate(
        &self,
        dag_id: &str,
        node_id: &str,
        kind: WaitGateKind,
        description: Option<String>,
        params: HashMap<String, serde_json::Value>,
        author: &str,
    ) -> Result<String> {
        let gate_id = mint_wait_gate_id();
        let delta = format!(
            "wait-gate add: {} on {}{}",
            kind,
            node_id,
            description
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        );
        self.plan_refine(
            dag_id,
            &delta,
            author,
            vec![DagMutation::AddWaitGate {
                node_id: node_id.to_string(),
                gate_id: Some(gate_id.clone()),
                kind,
                description,
                params,
            }],
        )
        .await?;
        Ok(gate_id)
    }

    /// Resolve a node wait-gate (emits `DagNodeWaitGateResolved`). Manual
    /// gates require an explicit non-gate resolver; the event is attributed
    /// to that actor. A gate already resolved ok/skipped cannot be resolved
    /// again; a failed gate can be re-resolved.
    pub async fn resolve_node_wait_gate(
        &self,
        dag_id: &str,
        node_id: &str,
        gate_id: &str,
        outcome: GateOutcome,
        resolver: Option<Actor>,
        reason: Option<String>,
    ) -> Result<()> {
        let scope = EventScope {
            dag_id: Some(dag_id.to_string()),
            node_id: Some(node_id.to_string()),
            ..Default::default()
        };
        self.ensure_policy_scope(&scope).await?;
        let dag_events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&dag_events, dag_id);
        let deny = |code: &str, reason: String| {
            Err(anyhow::Error::from(
                GateError::new(gate_ids::WAIT_GATE, code, reason)
                    .at(dag_id, Some(node_id))
                    .with_details(json!({ "gate_id": gate_id })),
            ))
        };
        let Some(node) = dag.nodes.get(node_id) else {
            return deny("node_not_found", format!("node {node_id} not in {dag_id}"));
        };
        let Some(gate) = node.wait_gates.iter().find(|g| g.gate_id == gate_id) else {
            return deny(
                "gate_not_found",
                format!("wait-gate {gate_id} not on node {node_id}"),
            );
        };
        if gate.is_resolved_ok() {
            return deny(
                "already_resolved",
                format!(
                    "wait-gate {gate_id} already resolved {}",
                    gate.outcome.map(|o| o.to_string()).unwrap_or_default()
                ),
            );
        }
        let explicit = resolver
            .as_ref()
            .filter(|a| a.r#type != ActorType::Gate && !a.id.trim().is_empty());
        if gate.kind == WaitGateKind::Manual && explicit.is_none() {
            return deny(
                "manual_resolve_requires_actor",
                "manual wait-gates must be resolved by an explicit user/agent actor".to_string(),
            );
        }
        let actor = explicit.cloned().unwrap_or_else(|| gate_actor(&self.actor_id));
        let evt = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: actor.clone(),
            scope: Some(scope),
            r#type: "DagNodeWaitGateResolved".to_string(),
            payload: json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "gate_id": gate_id,
                "kind": gate.kind,
                "outcome": outcome,
                "resolved_by": actor_label(&actor),
                "reason": reason
            }),
            provenance: None,
        };
        self.emit(evt).await?;
        self.refresh_dag_view(dag_id).await?;
        Ok(())
    }

    /// Lazy timer resolution: record `DagNodeWaitGateResolved` (outcome ok,
    /// actor gate) for every unresolved timer gate in the dag whose `until`
    /// has passed. Called on readiness checks (pickup, `wih list --ready`).
    /// Returns the resolved gate ids.
    pub async fn resolve_elapsed_timer_gates(&self, dag_id: &str) -> Result<Vec<String>> {
        let dag_events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&dag_events, dag_id);
        let now = Utc::now();
        let mut resolved = Vec::new();
        let mut node_ids: Vec<&String> = dag.nodes.keys().collect();
        node_ids.sort();
        for node_id in node_ids {
            let node = &dag.nodes[node_id];
            for gate in node.wait_gates.iter().filter(|g| g.timer_elapsed(now)) {
                let actor = gate_actor(&self.actor_id);
                let evt = AllternitEvent {
                    event_id: create_event_id(),
                    ts: now.to_rfc3339(),
                    actor: actor.clone(),
                    scope: None,
                    r#type: "DagNodeWaitGateResolved".to_string(),
                    payload: json!({
                        "dag_id": dag_id,
                        "node_id": node_id,
                        "gate_id": gate.gate_id,
                        "kind": gate.kind,
                        "outcome": GateOutcome::Ok,
                        "resolved_by": actor_label(&actor),
                        "reason": "timer elapsed"
                    }),
                    provenance: None,
                };
                self.emit(evt).await?;
                resolved.push(gate.gate_id.clone());
            }
        }
        if !resolved.is_empty() {
            self.refresh_dag_view(dag_id).await?;
        }
        Ok(resolved)
    }

    /// Store a node's output: blob + `node.output` receipt + ReceiptWritten,
    /// the derived `.out.md` view, and `DagNodeOutputRecorded`.
    async fn record_node_output(
        &self,
        wih_id: &str,
        dag_id: &str,
        node_id: &str,
        text: &str,
    ) -> Result<String> {
        let blob_id = self.receipts.store_blob_string(text)?;
        let sha256 = crate::context::types::sha256_with_prefix(text);
        let size_bytes = text.len() as u64;
        let output_path = node_output_rel_path(dag_id, node_id);
        let receipt_id = create_receipt_id();
        let receipt = ReceiptRecord {
            receipt_id: receipt_id.clone(),
            run_id: format!("run_{}", wih_id),
            step: None,
            tool: "node.output".to_string(),
            tool_version: None,
            inputs_ref: None,
            outputs_ref: Some(format!("blob:{blob_id}")),
            exit: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
        };
        self.receipts.write_receipt(&receipt)?;
        self.emit(AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "ReceiptWritten".to_string(),
            payload: json!({
                "wih_id": wih_id,
                "dag_id": dag_id,
                "node_id": node_id,
                "receipt_id": receipt_id,
                "tool": "node.output",
                "payload": {
                    "blob_id": blob_id,
                    "sha256": sha256,
                    "size_bytes": size_bytes,
                    "output_path": output_path
                }
            }),
            provenance: None,
        })
        .await?;
        let view = self.root_dir.join(&output_path);
        if let Some(parent) = view.parent() {
            ensure_dir(parent)?;
        }
        std::fs::write(&view, text)?;
        self.emit(AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "DagNodeOutputRecorded".to_string(),
            payload: json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "wih_id": wih_id,
                "receipt_id": receipt_id,
                "blob_id": blob_id,
                "sha256": sha256,
                "size_bytes": size_bytes,
                "output_path": output_path
            }),
            provenance: None,
        })
        .await?;
        Ok(receipt_id)
    }

    async fn resolve_prompt_id(&self, dag_id: &str) -> Result<String> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        for evt in events {
            if evt.r#type == "PromptLinkedToWork" {
                if evt.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id) {
                    if let Some(prompt_id) = evt.payload.get("prompt_id").and_then(|v| v.as_str()) {
                        return Ok(prompt_id.to_string());
                    }
                }
            }
        }
        Err(anyhow!("prompt not linked to dag"))
    }

    async fn ensure_no_cycle(&self, dag_id: &str, from: &str, to: &str) -> Result<()> {
        let events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&events, dag_id);
        if would_create_cycle(&dag.edges, from, to) {
            return Err(anyhow!("blocked_by edge would create cycle"));
        }
        Ok(())
    }

    fn ensure_mutation_provenance(&self, provenance: &MutationProvenance) -> Result<()> {
        if !self.strict_provenance {
            return Ok(());
        }
        let has_prompt = provenance.prompt_id.is_some() && provenance.delta_id.is_some();
        let has_decision = provenance.agent_decision_id.is_some();
        if !has_prompt && !has_decision {
            return Err(anyhow!("mutation requires prompt delta or agent decision"));
        }
        Ok(())
    }

    fn provenance_from(
        &self,
        provenance: &MutationProvenance,
    ) -> crate::core::types::EventProvenance {
        crate::core::types::EventProvenance {
            prompt_id: provenance.prompt_id.clone(),
            delta_id: provenance.delta_id.clone(),
            agent_decision_id: provenance.agent_decision_id.clone(),
            parent_event_id: None,
        }
    }

    async fn ensure_agent_decision_links(&self, decision_id: &str, event_id: &str) -> Result<()> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        for evt in events {
            if evt.r#type == "AgentDecisionRecorded" {
                if evt.payload.get("decision_id").and_then(|v| v.as_str()) == Some(decision_id) {
                    let linked = evt
                        .payload
                        .get("linked_event_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                        .unwrap_or_default();
                    if linked.iter().any(|id| *id == event_id) {
                        return Ok(());
                    }
                    return Err(anyhow!("agent decision missing linked event id"));
                }
            }
        }
        Err(anyhow!("agent decision not found"))
    }

    async fn active_wih_for_node(&self, node_id: &str) -> Result<Option<String>> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let mut node_map = std::collections::HashMap::new();
        let mut closed = std::collections::HashSet::new();
        for evt in &events {
            if evt.r#type == "WIHCreated" {
                if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                    if let Some(nid) = evt.payload.get("node_id").and_then(|v| v.as_str()) {
                        node_map.insert(wih_id.to_string(), nid.to_string());
                    }
                }
            }
            if evt.r#type == "WIHClosedSigned" {
                if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                    closed.insert(wih_id.to_string());
                }
            }
        }
        for (wih_id, nid) in node_map {
            if nid == node_id && !closed.contains(&wih_id) {
                return Ok(Some(wih_id));
            }
        }
        Ok(None)
    }

    fn allowed_tools_for_wih(&self, wih_id: &str, events: &[AllternitEvent]) -> Option<Vec<String>> {
        for evt in events {
            if evt.r#type == "WIHCreated" {
                if evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id) {
                    if let Some(policy) = evt.payload.get("policy") {
                        if let Some(tools) = policy.get("allowed_tools").and_then(|v| v.as_array())
                        {
                            return Some(
                                tools
                                    .iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect(),
                            );
                        }
                    }
                }
            }
        }
        None
    }

    async fn write_context_pack(
        &self,
        wih_id: &str,
        dag_id: &str,
        node_id: &str,
        resolved_description: Option<String>,
        fence: &Fence,
    ) -> Result<()> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let dag_events = events_for_dag(&events, dag_id);
        let dag = project_dag(&dag_events, dag_id);

        let ancestors = collect_ancestor_chain(&dag, node_id);
        let deps = collect_blocked_by_predecessors(&dag, node_id);
        let mut nodeset = std::collections::HashSet::new();
        nodeset.insert(node_id.to_string());
        nodeset.extend(ancestors.iter().cloned());
        nodeset.extend(deps.iter().cloned());

        for rel in dag.relations.iter().filter(|r| r.context_share) {
            if nodeset.contains(&rel.a) && !nodeset.contains(&rel.b) {
                nodeset.insert(rel.b.clone());
            } else if nodeset.contains(&rel.b) && !nodeset.contains(&rel.a) {
                nodeset.insert(rel.a.clone());
            }
        }

        let nodes: Vec<DagNode> = dag
            .nodes
            .values()
            .filter(|n| nodeset.contains(&n.node_id))
            .cloned()
            .collect();
        let edges: Vec<DagEdge> = dag
            .edges
            .iter()
            .filter(|e| nodeset.contains(&e.from_node_id) && nodeset.contains(&e.to_node_id))
            .cloned()
            .collect();
        let relations: Vec<DagRelation> = dag
            .relations
            .iter()
            .filter(|r| nodeset.contains(&r.a) && nodeset.contains(&r.b))
            .cloned()
            .collect();

        let prompt_id = self.resolve_prompt_id(dag_id).await.ok();
        let prompt = prompt_id
            .as_deref()
            .and_then(|pid| project_prompt(&events, pid));

        let receipt_refs = collect_receipts_for_nodes(&events, dag_id, &deps);
        let mut dep_ids: Vec<&String> = deps.iter().collect();
        dep_ids.sort();
        let mut dependency_outputs = Vec::new();
        for dep in dep_ids {
            let Some(output) = dag.nodes.get(dep).and_then(|n| n.output.as_ref()) else {
                continue;
            };
            let (text, truncated) = match self.read_node_output(output) {
                Ok(full) => {
                    let (cut, truncated) = truncate_utf8(&full, CONTEXT_PACK_OUTPUT_INLINE_CAP);
                    (Some(fence.wrap(&format!("node:{dep}"), cut)), truncated)
                }
                Err(_) => (None, false),
            };
            dependency_outputs.push(ContextNodeOutput {
                node_id: dep.clone(),
                wih_id: output.wih_id.clone(),
                receipt_id: output.receipt_id.clone(),
                blob_id: output.blob_id.clone(),
                sha256: output.sha256.clone(),
                size_bytes: output.size_bytes,
                output_path: output.output_path.clone(),
                text,
                truncated,
            });
        }
        let dag_slice = DagSlice {
            nodes,
            edges,
            relations,
        };
        let pack = ContextPack {
            version: "v1".to_string(),
            generated_at: Utc::now().to_rfc3339(),
            wih_id: wih_id.to_string(),
            dag_id: dag_id.to_string(),
            node_id: node_id.to_string(),
            execution_mode: "fresh".to_string(),
            prompt,
            dag_slice,
            dependency_nodes: deps.into_iter().collect(),
            receipts: receipt_refs,
            dependency_outputs,
            resolved_description,
            untrusted_fence: ContextFence {
                nonce: fence.nonce().to_string(),
                instruction: fence.instruction(),
            },
        };

        let path = context_pack_path(&self.root_dir, dag_id, wih_id);
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        write_json_atomic(&path, &pack)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DagMutation {
    CreateNode {
        node_id: String,
        node_kind: String,
        title: String,
        parent_node_id: Option<String>,
        execution_mode: String,
        /// Node prompt. May contain `{{ <node_id>.output }}` /
        /// `{{ <node_id>.output_path }}` placeholders (resolved at pickup).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// `bot:<slug>` | `ao:<harness>`; acted on only by `drive`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executor: Option<String>,
    },
    UpdateNode {
        node_id: String,
        patch: serde_json::Value,
    },
    DeleteNode {
        node_id: String,
    },
    ReparentNode {
        node_id: String,
        new_parent_id: Option<String>,
    },
    AddBlockedBy {
        from_node_id: String,
        to_node_id: String,
    },
    AddRelation {
        a: String,
        b: String,
        note: Option<String>,
        context_share: Option<bool>,
    },
    AddLabel {
        node_id: String,
        label: String,
    },
    RemoveLabel {
        node_id: String,
        label: String,
    },
    SetState {
        node_id: String,
        dimension: String,
        value: String,
        reason: Option<String>,
    },
    AddComment {
        node_id: String,
        body_ref: String,
        author: String,
    },
    ChangeStatus {
        node_id: String,
        from: String,
        to: String,
        reason: Option<String>,
    },
    /// Attach a wait-gate to a node (emits `DagNodeWaitGateAdded`).
    AddWaitGate {
        node_id: String,
        /// Minted when absent.
        #[serde(default)]
        gate_id: Option<String>,
        kind: WaitGateKind,
        #[serde(default)]
        description: Option<String>,
        /// Kind-specific params: `until` (timer, RFC 3339), `repo`,
        /// `run_id`, `pr`.
        #[serde(default)]
        params: HashMap<String, serde_json::Value>,
    },
}

fn context_pack_path(root: &PathBuf, dag_id: &str, wih_id: &str) -> PathBuf {
    root.join(".allternit/work/dags")
        .join(dag_id)
        .join("wih")
        .join("context")
        .join(format!("{wih_id}.context.json"))
}

fn resolved_prompt_path(root: &std::path::Path, dag_id: &str, wih_id: &str) -> PathBuf {
    root.join(".allternit/work/dags")
        .join(dag_id)
        .join("wih")
        .join("context")
        .join(format!("{wih_id}.prompt.md"))
}

/// Workspace-relative path of a node's derived output view.
pub fn node_output_rel_path(dag_id: &str, node_id: &str) -> String {
    format!(".allternit/work/dags/{dag_id}/nodes/{node_id}.out.md")
}

/// `s` cut to at most `max` bytes on a char boundary, and whether it was cut.
fn truncate_utf8(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

fn actor_label(actor: &Actor) -> String {
    let kind = match actor.r#type {
        ActorType::User => "user",
        ActorType::Agent => "agent",
        ActorType::Gate => "gate",
    };
    format!("{kind}:{}", actor.id)
}

fn mint_wait_gate_id() -> String {
    format!("wg_{:06x}", rand::random::<u32>() & 0x00ff_ffff)
}

fn events_for_dag(events: &[AllternitEvent], dag_id: &str) -> Vec<AllternitEvent> {
    events
        .iter()
        .filter(|evt| evt.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .cloned()
        .collect()
}

fn collect_ancestor_chain(dag: &DagState, node_id: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut current = node_id.to_string();
    while let Some(node) = dag.nodes.get(&current) {
        if let Some(parent) = &node.parent_node_id {
            if out.insert(parent.clone()) {
                current = parent.clone();
                continue;
            }
        }
        break;
    }
    out
}

fn collect_blocked_by_predecessors(
    dag: &DagState,
    node_id: &str,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut stack = vec![node_id.to_string()];
    while let Some(target) = stack.pop() {
        for edge in dag
            .edges
            .iter()
            .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == target)
        {
            if out.insert(edge.from_node_id.clone()) {
                stack.push(edge.from_node_id.clone());
            }
        }
    }
    out
}

fn collect_receipts_for_nodes(
    events: &[AllternitEvent],
    dag_id: &str,
    node_ids: &std::collections::HashSet<String>,
) -> Vec<ContextReceipt> {
    let mut wih_to_node = std::collections::HashMap::new();
    for evt in events {
        if evt.r#type == "WIHCreated" {
            if evt.payload.get("dag_id").and_then(|v| v.as_str()) != Some(dag_id) {
                continue;
            }
            if let (Some(wih_id), Some(node_id)) = (
                evt.payload.get("wih_id").and_then(|v| v.as_str()),
                evt.payload.get("node_id").and_then(|v| v.as_str()),
            ) {
                wih_to_node.insert(wih_id.to_string(), node_id.to_string());
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for evt in events {
        if evt.r#type != "ReceiptWritten" {
            continue;
        }
        let wih_id = match evt.payload.get("wih_id").and_then(|v| v.as_str()) {
            Some(value) => value,
            None => continue,
        };
        let node_id = wih_to_node.get(wih_id).cloned();
        if let Some(node_id) = &node_id {
            if !node_ids.contains(node_id) {
                continue;
            }
        } else {
            continue;
        }
        let receipt_id = match evt.payload.get("receipt_id").and_then(|v| v.as_str()) {
            Some(value) => value,
            None => continue,
        };
        if seen.insert(receipt_id.to_string()) {
            out.push(ContextReceipt {
                receipt_id: receipt_id.to_string(),
                wih_id: wih_id.to_string(),
                node_id,
            });
        }
    }
    out
}

fn gate_actor(actor_id: &str) -> Actor {
    Actor {
        r#type: ActorType::Gate,
        id: actor_id.to_string(),
    }
}
