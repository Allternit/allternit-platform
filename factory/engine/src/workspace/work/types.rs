use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::wait_gates::{GateOutcome, WaitGateKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub node_id: String,
    pub dag_id: String,
    pub parent_node_id: Option<String>,
    pub node_kind: String,
    pub title: String,
    pub description: Option<String>,
    pub execution_mode: String,
    pub owner_role: Option<String>,
    pub priority: Option<i64>,
    pub labels: Vec<String>,
    pub status: String,
    pub current_wih_id: Option<String>,
    pub assignee: Option<String>,
    pub spec_id: Option<String>,
    pub notes: Option<String>,
    pub acceptance: Option<String>,
    pub design: Option<String>,
    pub state: HashMap<String, String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// Worktree configuration for this node
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeConfig>,
    /// Who should execute this node: `bot:<slug>` or `ao:<harness>`.
    /// Acted on only by the opt-in `drive` command (spec/DRIVE.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// Latest recorded node output (`DagNodeOutputRecorded`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<NodeOutputRef>,
    /// Node-scoped wait-gates (`DagNodeWaitGateAdded` / `DagNodeWaitGateResolved`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wait_gates: Vec<NodeWaitGate>,
}

/// Reference to a node's recorded output text. The content lives in an
/// immutable blob (`.allternit/blobs/<blob_id>`) behind a receipt;
/// `output_path` is the derived, rebuildable view
/// `.allternit/work/dags/<dag_id>/nodes/<node_id>.out.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeOutputRef {
    pub wih_id: String,
    pub receipt_id: String,
    pub blob_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// Path relative to the workspace root.
    pub output_path: String,
    pub recorded_at: String,
}

/// A wait-gate attached to a DAG node. The node is not ready (and Gate 1
/// refuses pickup) while any of its gates is unsatisfied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeWaitGate {
    pub gate_id: String,
    pub kind: WaitGateKind,
    pub description: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<GateOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    /// `<actor_type>:<actor_id>` of whoever resolved the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl NodeWaitGate {
    /// True when the gate no longer blocks the node at `now`.
    ///
    /// Ok/Skipped outcomes satisfy; Failed blocks until re-resolved. An
    /// unresolved Timer gate is satisfied once `params.until` has passed
    /// (lazy resolution: the gate records the resolution event on the next
    /// readiness check). Unresolved Manual/GitHub gates always block.
    pub fn is_satisfied(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        match self.outcome {
            Some(GateOutcome::Ok) | Some(GateOutcome::Skipped) => true,
            Some(GateOutcome::Failed) => false,
            None => self.timer_elapsed(now),
        }
    }

    /// True for an unresolved timer gate whose `until` has passed.
    pub fn timer_elapsed(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.kind == WaitGateKind::Timer
            && self.outcome.is_none()
            && self
                .params
                .get("until")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                .is_some_and(|until| now >= until)
    }

    /// True when a recorded resolution satisfies the gate (clock-free).
    pub fn is_resolved_ok(&self) -> bool {
        matches!(
            self.outcome,
            Some(GateOutcome::Ok) | Some(GateOutcome::Skipped)
        )
    }
}

/// Validate a node `executor`: `bot:<slug>` or `ao:<harness>`, where the
/// name is non-empty `[A-Za-z0-9_.-]`.
pub fn validate_executor(executor: &str) -> Result<(), String> {
    let (prefix, name) = executor
        .split_once(':')
        .ok_or_else(|| format!("executor {executor:?} must be bot:<slug> or ao:<harness>"))?;
    if prefix != "bot" && prefix != "ao" {
        return Err(format!(
            "executor {executor:?} must start with bot: or ao:"
        ));
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "executor {executor:?} needs a name of [A-Za-z0-9_.-] after {prefix}:"
        ));
    }
    Ok(())
}

impl DagNode {
    /// Wait-gates that still block this node at `now`.
    pub fn blocking_wait_gates(&self, now: chrono::DateTime<chrono::Utc>) -> Vec<&NodeWaitGate> {
        self.wait_gates
            .iter()
            .filter(|g| !g.is_satisfied(now))
            .collect()
    }
}

/// Worktree configuration for DAG nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Whether to auto-create a worktree
    #[serde(default = "default_true")]
    pub auto_create: bool,
    /// Branch name prefix
    #[serde(default = "default_worktree_prefix")]
    pub branch_prefix: String,
    /// Cleanup policy when node is done
    #[serde(default)]
    pub cleanup_on_done: CleanupPolicy,
}

fn default_true() -> bool {
    true
}

fn default_worktree_prefix() -> String {
    "agent/".to_string()
}

/// Cleanup policy for worktrees
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    /// Cleanup when node is done
    #[default]
    OnDone,
    /// Cleanup when entire DAG is complete
    OnDagComplete,
    /// Never cleanup automatically
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEdge {
    pub from_node_id: String,
    pub to_node_id: String,
    pub edge_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagRelation {
    pub a: String,
    pub b: String,
    pub note: Option<String>,
    pub context_share: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagState {
    pub dag_id: String,
    pub nodes: HashMap<String, DagNode>,
    pub edges: Vec<DagEdge>,
    pub relations: Vec<DagRelation>,
}
