use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActorType {
    User,
    Agent,
    Gate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub r#type: ActorType,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventScope {
    pub project_id: Option<String>,
    pub dag_id: Option<String>,
    pub node_id: Option<String>,
    pub wih_id: Option<String>,
    pub run_id: Option<String>,
    /// Team workspace this event belongs to (cowork-team integration)
    pub team_workspace_id: Option<String>,
    /// Human-readable team name for display purposes
    pub team_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventProvenance {
    pub prompt_id: Option<String>,
    pub delta_id: Option<String>,
    pub agent_decision_id: Option<String>,
    pub parent_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllternitEvent {
    pub event_id: String,
    pub ts: String,
    pub actor: Actor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<EventScope>,
    pub r#type: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<EventProvenance>,
}

#[derive(Debug, Clone, Default)]
pub struct LedgerQuery {
    pub r#type: Option<String>,
    pub types: Option<Vec<String>>,
    pub scope: Option<EventScope>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptExit {
    pub code: Option<i32>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptRecord {
    pub receipt_id: String,
    pub run_id: String,
    pub step: Option<i32>,
    pub tool: String,
    pub tool_version: Option<String>,
    pub inputs_ref: Option<String>,
    pub outputs_ref: Option<String>,
    pub exit: Option<ReceiptExit>,
    // --- Ported from mcp-agent tracing ---
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub lease_id: String,
    pub wih_id: String,
    pub agent_id: String,
    pub paths: Vec<String>,
    pub requested_at: String,
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub lease_id: String,
    pub wih_id: String,
    pub agent_id: String,
    pub paths: Vec<String>,
    pub requested_at: String,
    pub ttl_seconds: Option<i64>,
    pub status: String,
    pub granted_until: Option<String>,
    pub denied_reason: Option<String>,
}

// --- NEW MCP AGENT FEATURES ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    pub elicitation_id: String,
    pub message: String,
    pub requested_schema: Option<Value>,
    pub url: Option<String>,
    pub server_name: Option<String>,
    pub timeout_seconds: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResult {
    pub elicitation_id: String,
    pub status: String, // "approved", "declined", "cancelled"
    pub response: Option<Value>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingRequest {
    pub sampling_id: String,
    pub messages: Vec<Value>, // Array of MCP Messages
    pub system_prompt: Option<String>,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingResult {
    pub sampling_id: String,
    pub role: String,
    pub content: Value,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandoffRequest {
    pub source_agent_id: String,
    pub target_agent_id: String,
    pub reason: Option<String>,
    pub context_variables: Option<Value>,
}

// --- OBSERVABILITY & SESSION PROXYING (Ported from mcp-agent) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityMetadata {
    pub agent_name: Option<String>,
    pub model_name: Option<String>,
    pub operation_name: Option<String>,
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub tool_name: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionProxyRecord {
    pub session_id: String,
    pub upstream_actor: Actor,
    pub downstream_targets: Vec<String>, // List of tool/agent IDs
    pub created_at: String,
    pub last_active_at: String,
    pub metadata: Option<Value>,
}

pub mod semconv {
    pub const GEN_AI_AGENT_NAME: &str = "gen_ai.agent.name";
    pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
    pub const GEN_AI_USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
    pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
    pub const MCP_SESSION_ID: &str = "mcp.session.id";
    pub const MCP_TOOL_NAME: &str = "mcp.tool.name";
}

// --- OAUTH & IDENTITY (Ported from mcp-agent) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenRecord {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub token_type: String,
    pub scope: Option<String>,
    pub server_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIdentity {
    pub subject: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub provider: String,
    pub metadata: Option<Value>,
}
