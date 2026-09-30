use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopPolicy {
    pub mode: String,
    pub max_iterations: u32,
    pub fresh_every: Option<u32>,
    pub spawn_on: Vec<String>,
    pub spawn_delay_secs: Option<u64>,
    pub acceptance_refs: Vec<String>,
    pub required_evidence: Vec<String>,
    pub escalate_on_max_iterations: bool,
    /// Whether to automatically land the implementation on a PASS status
    pub autoland_on_pass: bool,
}

impl Default for LoopPolicy {
    fn default() -> Self {
        Self {
            mode: "ralph".to_string(),
            max_iterations: 6,
            fresh_every: Some(2),
            spawn_on: vec!["blocked".to_string(), "evidence_missing".to_string()],
            spawn_delay_secs: Some(15),
            acceptance_refs: Vec::new(),
            required_evidence: vec!["receipt.tool_calls".to_string()],
            escalate_on_max_iterations: true,
            autoland_on_pass: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WihLoopState {
    pub current_iteration: u32,
    pub last_started_at: Option<String>,
    pub last_completed_at: Option<String>,
    pub last_outcome: Option<String>,
    pub escalated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihState {
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub status: String,
    pub open_signed: bool,
    pub agent_id: Option<String>,
    pub role: Option<String>,
    pub picked_up_at: Option<String>,
    pub last_heartbeat: Option<String>,
    pub open_signature: Option<String>,
    pub close_request: Option<Value>,
    pub final_status: Option<String>,
    pub closed_at: Option<String>,
    pub execution_mode: Option<String>,
    pub context_pack_path: Option<String>,
    /// Node description with `{{ <node>.output }}` placeholders resolved at
    /// pickup (derived view, written only when the node had placeholders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_prompt_path: Option<String>,
    pub loop_policy: Option<LoopPolicy>,
    pub loop_state: Option<WihLoopState>,
    /// Pending elicitation request ID
    pub pending_elicitation: Option<String>,
    /// Pending sampling request ID
    pub pending_sampling: Option<String>,
    /// Terminal context for workspace integration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_context: Option<TerminalContext>,
}

/// Terminal context for WIH execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalContext {
    /// Session ID in workspace service
    pub session_id: String,
    /// Pane ID for this WIH
    pub pane_id: String,
    /// WebSocket endpoint for log streaming
    pub log_stream_endpoint: String,
    /// Worktree path (if using git worktrees)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
}
