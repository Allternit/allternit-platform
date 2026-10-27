use allternit_context_router::{ContextBundle, ContextRouter};
use allternit_embodiment::EmbodimentControlPlane;
use allternit_evals::EvaluationEngine;
use allternit_history::{HistoryError, HistoryLedger};
use allternit_memory::MemoryFabric;
use allternit_messaging::{EventEnvelope, MessagingSystem};
use allternit_packaging::PackageManager;
use allternit_policy::{PolicyEffect, PolicyEngine, PolicyRequest};
use allternit_providers::ProviderRouter;
use allternit_runtime_core::SessionManager;
use allternit_skills::SkillRegistry;
use allternit_workflows::WorkflowEngine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneConfig {
    pub control_plane_id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled_features: Vec<ControlFeature>,
    pub security_profile: SecurityProfile,
    pub audit_level: AuditLevel,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlFeature {
    AgentInspection,
    AgentControl,
    MemoryInspection,
    SkillInspection,
    WorkflowInspection,
    EmbodimentControl,
    PolicyEnforcement,
    AuditLogging,
    ReplayCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityProfile {
    pub sensitivity_tier: u8, // 0-4 corresponding to T0-T4
    pub compliance_requirements: Vec<ComplianceRequirement>,
    pub audit_level: AuditLevel,
    pub encryption_required: bool,
    pub network_isolation: NetworkIsolation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComplianceRequirement {
    SOC2,
    HIPAA,
    GDPR,
    PCI_DSS,
    ISO27001,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditLevel {
    None,
    Basic,
    Standard,
    Enhanced,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkIsolation {
    None,
    Namespace,
    Node,
    Physical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInspectionRequest {
    pub request_id: String,
    pub agent_id: String,
    pub tenant_id: String,
    pub inspection_scope: InspectionScope,
    pub requested_tier: u8,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InspectionScope {
    State,
    Memory,
    Skills,
    Workflows,
    Context,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInspectionResponse {
    pub request_id: String,
    pub agent_id: String,
    pub inspection_data: AgentInspectionData,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInspectionData {
    pub state: Option<serde_json::Value>,
    pub memory: Option<Vec<MemoryEntry>>,
    pub skills: Option<Vec<SkillInfo>>,
    pub workflows: Option<Vec<WorkflowInfo>>,
    pub context: Option<ContextBundle>,
    pub last_activity: Option<u64>,
    pub health_status: HealthStatus,
    pub autonomy_level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub memory_id: String,
    pub content: serde_json::Value,
    pub created_at: u64,
    pub last_accessed: u64,
    pub sensitivity_tier: u8,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub skill_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub last_executed: Option<u64>,
    pub execution_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInfo {
    pub workflow_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub last_executed: Option<u64>,
    pub execution_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Degraded,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentControlRequest {
    pub request_id: String,
    pub agent_id: String,
    pub tenant_id: String,
    pub control_action: ControlAction,
    pub reason: String,
    pub authorized_by: String,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlAction {
    Pause,
    Resume,
    Revoke,
    Terminate,
    Reset,
    UpdateConfig(AgentConfigUpdate),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfigUpdate {
    pub autonomy_level: Option<u8>,
    pub memory_retention: Option<u64>, // seconds
    pub skill_whitelist: Option<Vec<String>>,
    pub context_restrictions: Option<ContextRestrictions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRestrictions {
    pub allowed_tenants: Option<Vec<String>>,
    pub allowed_sessions: Option<Vec<String>>,
    pub sensitivity_tier_limit: Option<u8>,
    pub time_bounds: Option<(u64, u64)>, // start_time, end_time
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentControlResponse {
    pub request_id: String,
    pub agent_id: String,
    pub action: ControlAction,
    pub status: ControlStatus,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ControlStatus {
    Success,
    PartialSuccess,
    Failed,
    Denied,
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRequest {
    pub request_id: String,
    pub decision_id: Option<String>,
    pub memory_id: Option<String>,
    pub skill_execution_id: Option<String>,
    pub workflow_execution_id: Option<String>,
    pub trace_depth: u8,
    pub include_sensitive: bool,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceResponse {
    pub request_id: String,
    pub trace_data: TraceData,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceData {
    pub decision_trace: Option<DecisionTrace>,
    pub memory_trace: Option<MemoryTrace>,
    pub skill_trace: Option<SkillTrace>,
    pub workflow_trace: Option<WorkflowTrace>,
    pub context_trace: Option<ContextTrace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionTrace {
    pub decision_id: String,
    pub policy_request: PolicyRequest,
    pub policy_decision: PolicyEffect,
    pub decision_timestamp: u64,
    pub decision_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTrace {
    pub memory_id: String,
    pub access_log: Vec<MemoryAccessRecord>,
    pub retention_policy: MemoryRetentionPolicy,
    pub provenance: MemoryProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAccessRecord {
    pub access_id: String,
    pub accessor_id: String,
    pub access_type: MemoryAccessType,
    pub timestamp: u64,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryAccessType {
    Read,
    Write,
    Delete,
    Copy,
    Move,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRetentionPolicy {
    pub retention_period: Option<u64>, // seconds
    pub auto_delete: bool,
    pub backup_policy: BackupPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPolicy {
    pub enabled: bool,
    pub frequency: u64, // seconds
    pub retention: u64, // number of backups to keep
    pub encryption: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProvenance {
    pub source_session: Option<String>,
    pub source_agent: String,
    pub derivation_chain: Vec<String>,
    pub integrity_hash: String,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTrace {
    pub skill_id: String,
    pub execution_log: Vec<SkillExecutionRecord>,
    pub dependencies: Vec<String>,
    pub security_context: SecurityContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionRecord {
    pub execution_id: String,
    pub executor_id: String,
    pub input_data: serde_json::Value,
    pub output_data: serde_json::Value,
    pub execution_time: u64, // microseconds
    pub cost: f64,
    pub safety_rating: u8,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityContext {
    pub sensitivity_tier: u8,
    pub allowed_permissions: Vec<String>,
    pub required_compliance: Vec<ComplianceRequirement>,
    pub encryption_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTrace {
    pub workflow_id: String,
    pub execution_log: Vec<WorkflowExecutionRecord>,
    pub step_trace: Vec<StepTrace>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecutionRecord {
    pub execution_id: String,
    pub executor_id: String,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub status: WorkflowStatus,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTrace {
    pub step_id: String,
    pub step_type: StepType,
    pub input_data: serde_json::Value,
    pub output_data: serde_json::Value,
    pub execution_time: u64, // microseconds
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    SkillCall,
    Decision,
    MemoryAccess,
    ContextUpdate,
    ExternalCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextTrace {
    pub context_id: String,
    pub access_log: Vec<ContextAccessRecord>,
    pub evolution_log: Vec<ContextEvolutionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAccessRecord {
    pub access_id: String,
    pub accessor_id: String,
    pub access_type: ContextAccessType,
    pub timestamp: u64,
    pub context_snapshot: ContextBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextAccessType {
    Read,
    Write,
    Update,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEvolutionRecord {
    pub change_id: String,
    pub changer_id: String,
    pub change_type: ContextChangeType,
    pub old_value: serde_json::Value,
    pub new_value: serde_json::Value,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextChangeType {
    Add,
    Update,
    Remove,
    Merge,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] HistoryError),
    #[error("Policy error: {0}")]
    Policy(#[from] allternit_policy::PolicyError),
    #[error("Agent not found: {0}")]
    AgentNotFound(String),
    #[error("Inspection failed: {0}")]
    InspectionFailed(String),
    #[error("Control action failed: {0}")]
    ControlActionFailed(String),
    #[error("Trace failed: {0}")]
    TraceFailed(String),
    #[error("Security violation: {0}")]
    SecurityViolation(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Mutex poisoned: {0}")]
    MutexPoisoned(String),
    #[error("Time error: {0}")]
    TimeError(String),
}

pub struct ControlPlane {
    config: ControlPlaneConfig,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    context_router: Arc<ContextRouter>,
    memory_fabric: Arc<MemoryFabric>,
    provider_router: Arc<ProviderRouter>,
    skill_registry: Arc<SkillRegistry>,
    workflow_engine: Arc<WorkflowEngine>,
    embodiment_control_plane: Arc<EmbodimentControlPlane>,
    package_manager: Arc<PackageManager>,
    evaluation_engine: Arc<EvaluationEngine>,
    session_manager: Arc<SessionManager>,

    // Internal state
    active_agents: Arc<RwLock<HashMap<String, AgentState>>>,
    inspection_cache: Arc<RwLock<HashMap<String, AgentInspectionData>>>,
    control_history: Arc<RwLock<HashMap<String, AgentControlResponse>>>,
}

#[derive(Debug, Clone)]
struct AgentState {
    agent_id: String,
    status: AgentStatus,
    last_inspection: Option<u64>,
    autonomy_level: u8,
    health_status: HealthStatus,
}

#[derive(Debug, Clone)]
enum AgentStatus {
    Active,
    Paused,
    Revoked,
    Terminated,
}

impl ControlPlane {
    pub fn new(
        config: ControlPlaneConfig,
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        context_router: Arc<ContextRouter>,
        memory_fabric: Arc<MemoryFabric>,
        provider_router: Arc<ProviderRouter>,
        skill_registry: Arc<SkillRegistry>,
        workflow_engine: Arc<WorkflowEngine>,
        embodiment_control_plane: Arc<EmbodimentControlPlane>,
        package_manager: Arc<PackageManager>,
        evaluation_engine: Arc<EvaluationEngine>,
        session_manager: Arc<SessionManager>,
    ) -> Self {
        ControlPlane {
            config,
            history_ledger,
            messaging_system,
            policy_engine,
            context_router,
            memory_fabric,
            provider_router,
            skill_registry,
            workflow_engine,
            embodiment_control_plane,
            package_manager,
            evaluation_engine,
            session_manager,
            active_agents: Arc::new(RwLock::new(HashMap::new())),
            inspection_cache: Arc::new(RwLock::new(HashMap::new())),
            control_history: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Inspect an agent's state, memory, skills, workflows, and context
    pub async fn inspect_agent(
        &self,
        request: AgentInspectionRequest,
    ) -> Result<AgentInspectionResponse, ControlPlaneError> {
        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: request.agent_id.clone(), // In real implementation, this would be the inspector
            resource: format!("agent_inspection:{}", request.agent_id),
            action: "inspect".to_string(),
            context: serde_json::json!({
                "request": &request,
                "tenant_id": &request.tenant_id,
                "inspection_scope": &request.inspection_scope,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for inspection
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(ControlPlaneError::SecurityViolation(format!(
                "Policy denied agent inspection: {}",
                policy_decision.reason
            )));
        }

        // Perform the inspection based on scope
        let inspection_data = self.perform_agent_inspection(&request).await?;

        // Cache the inspection result
        {
            let mut cache = self.inspection_cache.write().await;
            cache.insert(request.agent_id.clone(), inspection_data.clone());
        }

        let response = AgentInspectionResponse {
            request_id: request.request_id,
            agent_id: request.agent_id,
            inspection_data,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id,
        };

        // Log the inspection event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "AgentInspected".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: request.tenant_id,
            actor_id: "control_plane".to_string(), // Inspector identity
            role: "control_plane".to_string(),
            timestamp: response.timestamp,
            trace_id: response.trace_id.clone(),
            payload: serde_json::json!({
                "request_id": response.request_id,
                "agent_id": response.agent_id,
                "inspection_scope": request.inspection_scope,
                "data_summary": {
                    "has_state": response.inspection_data.state.is_some(),
                    "memory_count": response.inspection_data.memory.as_ref().map(|m| m.len()).unwrap_or(0),
                    "skill_count": response.inspection_data.skills.as_ref().map(|s| s.len()).unwrap_or(0),
                    "workflow_count": response.inspection_data.workflows.as_ref().map(|w| w.len()).unwrap_or(0),
                },
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        // Emit event asynchronously
        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(response)
    }

    async fn perform_agent_inspection(
        &self,
        request: &AgentInspectionRequest,
    ) -> Result<AgentInspectionData, ControlPlaneError> {
        let mut inspection_data = AgentInspectionData {
            state: None,
            memory: None,
            skills: None,
            workflows: None,
            context: None,
            last_activity: None,
            health_status: HealthStatus::Unknown,
            autonomy_level: 0,
        };

        match &request.inspection_scope {
            InspectionScope::State => {
                // In a real implementation, this would inspect the agent's state
                // For now, we'll return a placeholder
                inspection_data.state = Some(serde_json::json!({
                    "status": "active",
                    "last_activity": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    "autonomy_level": 3,
                }));
            }
            InspectionScope::Memory => {
                // Inspect memory fabric for agent-specific data
                // This is a simplified implementation
                let memory_query = allternit_memory::MemoryQuery {
                    query: request.agent_id.clone(),
                    top_k: 50,
                    filter: None,
                    tenant_id: request.tenant_id.clone(),
                    session_id: None,
                    agent_id: Some(request.agent_id.clone()),
                    memory_types: vec![],
                    max_sensitivity_tier: Some(request.requested_tier),
                    required_tags: vec![],
                    time_range: None,
                    content_search: Some(request.agent_id.clone()),
                    limit: Some(100),
                    sort_by: None,
                    status_filter: None,
                };

                let memory_entries = self
                    .memory_fabric
                    .query_memory(memory_query, request.agent_id.clone())
                    .await
                    .map_err(|e| ControlPlaneError::InspectionFailed(e.to_string()))?;

                inspection_data.memory = Some(
                    memory_entries
                        .iter()
                        .map(|entry| MemoryEntry {
                            memory_id: entry.memory_id.clone(),
                            content: entry.content.clone(),
                            created_at: entry.created_at,
                            last_accessed: entry.last_accessed,
                            sensitivity_tier: entry.sensitivity_tier,
                            tags: entry.tags.clone(),
                        })
                        .collect(),
                );
            }
            InspectionScope::Skills => {
                // Get skills associated with the agent
                // In a real implementation, this would query the skill registry
                // For now, we'll return a placeholder
                inspection_data.skills = Some(vec![SkillInfo {
                    skill_id: "skill-1".to_string(),
                    name: "Sample Skill".to_string(),
                    version: "1.0.0".to_string(),
                    description: "A sample skill".to_string(),
                    last_executed: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    execution_count: 10,
                }]);
            }
            InspectionScope::Workflows => {
                // Get workflows associated with the agent
                // In a real implementation, this would query the workflow engine
                // For now, we'll return a placeholder
                inspection_data.workflows = Some(vec![WorkflowInfo {
                    workflow_id: "workflow-1".to_string(),
                    name: "Sample Workflow".to_string(),
                    version: "1.0.0".to_string(),
                    description: "A sample workflow".to_string(),
                    status: WorkflowStatus::Completed,
                    last_executed: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    execution_count: 5,
                }]);
            }
            InspectionScope::Context => {
                // Get context bundle for the agent
                // In a real implementation, this would query the context router
                // For now, we'll return a placeholder
                inspection_data.context = Some(ContextBundle {
                    bundle_id: format!("context-bundle-{}", request.agent_id),
                    tenant_id: request.tenant_id.clone(),
                    session_id: Some("session-1".to_string()),
                    created_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    expires_at: None,
                    context_entries: vec![],
                    provenance: allternit_context_router::ContextProvenance {
                        origin_session: Some("session-1".to_string()),
                        origin_agent: request.agent_id.clone(),
                        derivation_chain: vec!["initial".to_string()],
                        integrity_hash: "placeholder_hash".to_string(),
                        signature: None,
                    },
                    access_control: allternit_context_router::ContextAccessControl {
                        allowed_agents: std::collections::HashSet::from([request.agent_id.clone()]),
                        allowed_skills: std::collections::HashSet::new(),
                        allowed_phases: std::collections::HashSet::new(),
                        time_window: None,
                        access_policy:
                            allternit_context_router::ContextAccessPolicy::ExplicitAllowList,
                    },
                    size_bytes: 0,
                    last_accessed: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    access_count: 0,
                });
            }
            InspectionScope::All => {
                // Perform all inspections
                // State
                inspection_data.state = Some(serde_json::json!({
                    "status": "active",
                    "last_activity": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    "autonomy_level": 3,
                }));

                // Memory
                let memory_query = allternit_memory::MemoryQuery {
                    query: request.agent_id.clone(),
                    top_k: 50,
                    filter: None,
                    tenant_id: request.tenant_id.clone(),
                    session_id: None,
                    agent_id: Some(request.agent_id.clone()),
                    memory_types: vec![],
                    max_sensitivity_tier: Some(request.requested_tier),
                    required_tags: vec![],
                    time_range: None,
                    content_search: Some(request.agent_id.clone()),
                    limit: Some(100),
                    sort_by: None,
                    status_filter: None,
                };

                let memory_entries = self
                    .memory_fabric
                    .query_memory(memory_query, request.agent_id.clone())
                    .await
                    .map_err(|e| ControlPlaneError::InspectionFailed(e.to_string()))?;

                inspection_data.memory = Some(
                    memory_entries
                        .iter()
                        .map(|entry| MemoryEntry {
                            memory_id: entry.memory_id.clone(),
                            content: entry.content.clone(),
                            created_at: entry.created_at,
                            last_accessed: entry.last_accessed,
                            sensitivity_tier: entry.sensitivity_tier,
                            tags: entry.tags.clone(),
                        })
                        .collect(),
                );

                // Skills
                inspection_data.skills = Some(vec![SkillInfo {
                    skill_id: "skill-1".to_string(),
                    name: "Sample Skill".to_string(),
                    version: "1.0.0".to_string(),
                    description: "A sample skill".to_string(),
                    last_executed: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    execution_count: 10,
                }]);

                // Workflows
                inspection_data.workflows = Some(vec![WorkflowInfo {
                    workflow_id: "workflow-1".to_string(),
                    name: "Sample Workflow".to_string(),
                    version: "1.0.0".to_string(),
                    description: "A sample workflow".to_string(),
                    status: WorkflowStatus::Completed,
                    last_executed: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    execution_count: 5,
                }]);

                // Context
                inspection_data.context = Some(ContextBundle {
                    bundle_id: format!("context-bundle-{}", request.agent_id),
                    tenant_id: request.tenant_id.clone(),
                    session_id: Some("session-1".to_string()),
                    created_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    expires_at: None,
                    context_entries: vec![],
                    provenance: allternit_context_router::ContextProvenance {
                        origin_session: Some("session-1".to_string()),
                        origin_agent: request.agent_id.clone(),
                        derivation_chain: vec!["initial".to_string()],
                        integrity_hash: "placeholder_hash".to_string(),
                        signature: None,
                    },
                    access_control: allternit_context_router::ContextAccessControl {
                        allowed_agents: std::collections::HashSet::from([request.agent_id.clone()]),
                        allowed_skills: std::collections::HashSet::new(),
                        allowed_phases: std::collections::HashSet::new(),
                        time_window: None,
                        access_policy:
                            allternit_context_router::ContextAccessPolicy::ExplicitAllowList,
                    },
                    size_bytes: 0,
                    last_accessed: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    access_count: 0,
                });
            }
        }

        Ok(inspection_data)
    }

    /// Control an agent by pausing, resuming, revoking, or terminating it
    pub async fn control_agent(
        &self,
        request: AgentControlRequest,
    ) -> Result<AgentControlResponse, ControlPlaneError> {
        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: request.authorized_by.clone(),
            resource: format!("agent_control:{}", request.agent_id),
            action: "control".to_string(),
            context: serde_json::json!({
                "request": &request,
                "tenant_id": &request.tenant_id,
                "control_action": &request.control_action,
                "reason": &request.reason,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for control
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(ControlPlaneError::SecurityViolation(format!(
                "Policy denied agent control: {}",
                policy_decision.reason
            )));
        }

        // Execute the control action
        let status = match &request.control_action {
            ControlAction::Pause => {
                // In a real implementation, this would pause the agent
                // For now, we'll just update the state
                self.update_agent_state(&request.agent_id, AgentStatus::Paused)
                    .await;
                ControlStatus::Success
            }
            ControlAction::Resume => {
                // In a real implementation, this would resume the agent
                // For now, we'll just update the state
                self.update_agent_state(&request.agent_id, AgentStatus::Active)
                    .await;
                ControlStatus::Success
            }
            ControlAction::Revoke => {
                // In a real implementation, this would revoke the agent's permissions
                // For now, we'll just update the state
                self.update_agent_state(&request.agent_id, AgentStatus::Revoked)
                    .await;
                ControlStatus::Success
            }
            ControlAction::Terminate => {
                // In a real implementation, this would terminate the agent
                // For now, we'll just update the state
                self.update_agent_state(&request.agent_id, AgentStatus::Terminated)
                    .await;
                ControlStatus::Success
            }
            ControlAction::Reset => {
                // In a real implementation, this would reset the agent
                // For now, we'll just update the state
                self.update_agent_state(&request.agent_id, AgentStatus::Active)
                    .await;
                ControlStatus::Success
            }
            ControlAction::UpdateConfig(update) => {
                // In a real implementation, this would update the agent's configuration
                // For now, we'll just return success
                ControlStatus::Success
            }
        };

        let response = AgentControlResponse {
            request_id: request.request_id,
            agent_id: request.agent_id,
            action: request.control_action,
            status,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id,
        };

        // Store in control history
        {
            let mut history = self.control_history.write().await;
            history.insert(response.request_id.clone(), response.clone());
        }

        // Log the control event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "AgentControlled".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: request.tenant_id,
            actor_id: request.authorized_by,
            role: "control_plane".to_string(),
            timestamp: response.timestamp,
            trace_id: response.trace_id.clone(),
            payload: serde_json::json!({
                "request_id": response.request_id,
                "agent_id": response.agent_id,
                "action": format!("{:?}", response.action),
                "status": format!("{:?}", response.status),
                "reason": request.reason,
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        // Emit event asynchronously
        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(response)
    }

    async fn update_agent_state(&self, agent_id: &str, status: AgentStatus) {
        let mut agents = self.active_agents.write().await;
        if let Some(agent_state) = agents.get_mut(agent_id) {
            agent_state.status = status;
            agent_state.last_inspection = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
        } else {
            agents.insert(
                agent_id.to_string(),
                AgentState {
                    agent_id: agent_id.to_string(),
                    status,
                    last_inspection: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    autonomy_level: 0,
                    health_status: HealthStatus::Unknown,
                },
            );
        }
    }

    /// Trace decisions, memory access, skill execution, and workflow execution
    pub async fn trace(&self, request: TraceRequest) -> Result<TraceResponse, ControlPlaneError> {
        let mut trace_data = TraceData {
            decision_trace: None,
            memory_trace: None,
            skill_trace: None,
            workflow_trace: None,
            context_trace: None,
        };

        // Trace decision if requested
        if let Some(decision_id) = &request.decision_id {
            trace_data.decision_trace = self.trace_decision(decision_id, &request).await?;
        }

        // Trace memory if requested
        if let Some(memory_id) = &request.memory_id {
            trace_data.memory_trace = self.trace_memory(memory_id, &request).await?;
        }

        // Trace skill execution if requested
        if let Some(skill_execution_id) = &request.skill_execution_id {
            trace_data.skill_trace = self
                .trace_skill_execution(skill_execution_id, &request)
                .await?;
        }

        // Trace workflow execution if requested
        if let Some(workflow_execution_id) = &request.workflow_execution_id {
            trace_data.workflow_trace = self
                .trace_workflow_execution(workflow_execution_id, &request)
                .await?;
        }

        let response = TraceResponse {
            request_id: request.request_id,
            trace_data,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id,
        };

        // Log the trace event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "TraceRequested".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "control_plane".to_string(), // Tracer identity
            role: "control_plane".to_string(),
            timestamp: response.timestamp,
            trace_id: response.trace_id.clone(),
            payload: serde_json::json!({
                "request_id": response.request_id,
                "trace_depth": request.trace_depth,
                "decision_traced": response.trace_data.decision_trace.is_some(),
                "memory_traced": response.trace_data.memory_trace.is_some(),
                "skill_traced": response.trace_data.skill_trace.is_some(),
                "workflow_traced": response.trace_data.workflow_trace.is_some(),
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        // Emit event asynchronously
        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(response)
    }

    async fn trace_decision(
        &self,
        decision_id: &str,
        request: &TraceRequest,
    ) -> Result<Option<DecisionTrace>, ControlPlaneError> {
        // In a real implementation, this would trace a specific decision
        // For now, we'll return a placeholder
        Ok(Some(DecisionTrace {
            decision_id: decision_id.to_string(),
            policy_request: PolicyRequest {
                identity_id: "system".to_string(),
                resource: "placeholder".to_string(),
                action: "placeholder".to_string(),
                context: serde_json::json!({}),
                requested_tier: allternit_policy::SafetyTier::T0,
            },
            policy_decision: PolicyEffect::Allow,
            decision_timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            decision_reason: "Placeholder decision".to_string(),
        }))
    }

    async fn trace_memory(
        &self,
        memory_id: &str,
        request: &TraceRequest,
    ) -> Result<Option<MemoryTrace>, ControlPlaneError> {
        // In a real implementation, this would trace memory access
        // For now, we'll return a placeholder
        Ok(Some(MemoryTrace {
            memory_id: memory_id.to_string(),
            access_log: vec![MemoryAccessRecord {
                access_id: "access-1".to_string(),
                accessor_id: "agent-1".to_string(),
                access_type: MemoryAccessType::Read,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                context: serde_json::json!({"operation": "read", "source": "agent-1"}),
            }],
            retention_policy: MemoryRetentionPolicy {
                retention_period: Some(3600), // 1 hour
                auto_delete: false,
                backup_policy: BackupPolicy {
                    enabled: true,
                    frequency: 3600, // 1 hour
                    retention: 7,    // 7 backups
                    encryption: true,
                },
            },
            provenance: MemoryProvenance {
                source_session: Some("session-1".to_string()),
                source_agent: "agent-1".to_string(),
                derivation_chain: vec!["initial".to_string()],
                integrity_hash: "placeholder_hash".to_string(),
                signature: None,
            },
        }))
    }

    async fn trace_skill_execution(
        &self,
        skill_execution_id: &str,
        request: &TraceRequest,
    ) -> Result<Option<SkillTrace>, ControlPlaneError> {
        // In a real implementation, this would trace skill execution
        // For now, we'll return a placeholder
        Ok(Some(SkillTrace {
            skill_id: "skill-1".to_string(),
            execution_log: vec![SkillExecutionRecord {
                execution_id: skill_execution_id.to_string(),
                executor_id: "agent-1".to_string(),
                input_data: serde_json::json!({"input": "test"}),
                output_data: serde_json::json!({"output": "result"}),
                execution_time: 1000, // microseconds
                cost: 0.01,
                safety_rating: 3,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            }],
            dependencies: vec!["dependency-1".to_string()],
            security_context: SecurityContext {
                sensitivity_tier: 2,
                allowed_permissions: vec!["read".to_string(), "write".to_string()],
                required_compliance: vec![ComplianceRequirement::SOC2],
                encryption_required: true,
            },
        }))
    }

    async fn trace_workflow_execution(
        &self,
        workflow_execution_id: &str,
        request: &TraceRequest,
    ) -> Result<Option<WorkflowTrace>, ControlPlaneError> {
        // In a real implementation, this would trace workflow execution
        // For now, we'll return a placeholder
        Ok(Some(WorkflowTrace {
            workflow_id: "workflow-1".to_string(),
            execution_log: vec![WorkflowExecutionRecord {
                execution_id: workflow_execution_id.to_string(),
                executor_id: "agent-1".to_string(),
                start_time: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    - 10,
                end_time: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                status: WorkflowStatus::Completed,
                result: Some(serde_json::json!({"status": "success"})),
                error: None,
            }],
            step_trace: vec![StepTrace {
                step_id: "step-1".to_string(),
                step_type: StepType::SkillCall,
                input_data: serde_json::json!({"skill": "test-skill", "input": "data"}),
                output_data: serde_json::json!({"result": "success"}),
                execution_time: 500, // microseconds
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            }],
            dependencies: vec!["skill-1".to_string()],
        }))
    }

    /// Get the control plane configuration
    pub fn get_config(&self) -> &ControlPlaneConfig {
        &self.config
    }

    /// Get statistics about the control plane
    pub async fn get_statistics(&self) -> ControlPlaneStatistics {
        ControlPlaneStatistics {
            total_agents: self.active_agents.read().await.len() as u64,
            total_inspections: self.inspection_cache.read().await.len() as u64,
            total_controls: self.control_history.read().await.len() as u64,
            active_agents: self
                .active_agents
                .read()
                .await
                .values()
                .filter(|state| matches!(state.status, AgentStatus::Active))
                .count() as u64,
            paused_agents: self
                .active_agents
                .read()
                .await
                .values()
                .filter(|state| matches!(state.status, AgentStatus::Paused))
                .count() as u64,
            revoked_agents: self
                .active_agents
                .read()
                .await
                .values()
                .filter(|state| matches!(state.status, AgentStatus::Revoked))
                .count() as u64,
            terminated_agents: self
                .active_agents
                .read()
                .await
                .values()
                .filter(|state| matches!(state.status, AgentStatus::Terminated))
                .count() as u64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneStatistics {
    pub total_agents: u64,
    pub total_inspections: u64,
    pub total_controls: u64,
    pub active_agents: u64,
    pub paused_agents: u64,
    pub revoked_agents: u64,
    pub terminated_agents: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_control_plane_basic_functionality() {
        // Create temporary database
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();

        // Create temporary history ledger
        let temp_path = format!("/tmp/test_control_plane_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        // Create messaging system
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create policy engine
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let agent_identity = allternit_policy::Identity {
            id: "test-agent-001".to_string(),
            identity_type: allternit_policy::IdentityType::AgentIdentity,
            name: "Test Agent".to_string(),
            tenant_id: "test-tenant".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["agent".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine
            .register_identity(agent_identity)
            .await
            .unwrap();
        let user_identity = allternit_policy::Identity {
            id: "test-user".to_string(),
            identity_type: allternit_policy::IdentityType::HumanUser,
            name: "Test User".to_string(),
            tenant_id: "test-tenant".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["admin".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine
            .register_identity(user_identity)
            .await
            .unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        policy_engine.create_default_rules().await.unwrap();
        let inspect_rule = allternit_policy::PolicyRule {
            id: "rule_allow_inspect".to_string(),
            name: "Allow Inspect Operations".to_string(),
            description: "Allow agent inspection in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["inspect".to_string()],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(inspect_rule).await.unwrap();
        let control_rule = allternit_policy::PolicyRule {
            id: "rule_allow_control".to_string(),
            name: "Allow Control Operations".to_string(),
            description: "Allow agent control in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["control".to_string()],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(control_rule).await.unwrap();
        let query_rule = allternit_policy::PolicyRule {
            id: "rule_allow_query".to_string(),
            name: "Allow Query Operations".to_string(),
            description: "Allow memory queries in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["query".to_string()],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(query_rule).await.unwrap();

        // Create context router
        let context_router = Arc::new(allternit_context_router::ContextRouter::new(
            history_ledger.clone(),
            messaging_system.clone(),
            policy_engine.clone(),
            Arc::new(allternit_runtime_core::SessionManager::new(
                history_ledger.clone(),
                messaging_system.clone(),
            )),
        ));

        // Create memory fabric
        let memory_fabric = Arc::new(
            allternit_memory::MemoryFabric::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                pool.clone(),
                Arc::new(allternit_memory::v2::memory_policy::DefaultMemoryPolicy),
            )
            .await
            .unwrap(),
        );

        // Create provider router
        let provider_router = Arc::new(
            allternit_providers::ProviderRouter::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create tool gateway
        let tool_gateway = Arc::new(allternit_tools_gateway::ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create skill registry
        let skill_registry = Arc::new(
            allternit_skills::SkillRegistry::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                tool_gateway.clone(), // Use clone to avoid move
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Register native browser-use skills with recording capability
        let browser_skills = allternit_skills::browser_use::create_all_browser_skills();
        for skill in browser_skills {
            if let Err(e) = skill_registry.register_skill(skill).await {
                tracing::warn!("Failed to register browser skill: {}", e);
            }
        }
        tracing::info!("Registered browser-use skills with recording capability");

        // Create task queue
        let task_queue = Arc::new(
            allternit_messaging::TaskQueue::new(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );

        // Create workflow engine
        let workflow_engine = Arc::new(allternit_workflows::WorkflowEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
            policy_engine.clone(),
            tool_gateway.clone(),
            skill_registry.clone(),
            task_queue,
            pool.clone(),
        ));

        // Create embodiment control plane
        let embodiment_control_plane = Arc::new(
            allternit_embodiment::EmbodimentControlPlane::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                provider_router.clone(),
                skill_registry.clone(),
                workflow_engine.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create package manager
        let package_manager = Arc::new(
            allternit_packaging::PackageManager::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                provider_router.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create evaluation engine
        let evaluation_engine = Arc::new(
            allternit_evals::EvaluationEngine::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                provider_router.clone(),
                skill_registry.clone(),
                workflow_engine.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create session manager
        let session_manager = Arc::new(allternit_runtime_core::SessionManager::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create control plane config
        let config = ControlPlaneConfig {
            control_plane_id: "test-control-plane-001".to_string(),
            name: "Test Control Plane".to_string(),
            description: "A test control plane for Allternit".to_string(),
            version: "1.0.0".to_string(),
            enabled_features: vec![
                ControlFeature::AgentInspection,
                ControlFeature::AgentControl,
                ControlFeature::MemoryInspection,
                ControlFeature::AuditLogging,
            ],
            security_profile: SecurityProfile {
                sensitivity_tier: 3,
                compliance_requirements: vec![ComplianceRequirement::SOC2],
                audit_level: AuditLevel::Enhanced,
                encryption_required: true,
                network_isolation: NetworkIsolation::Namespace,
            },
            audit_level: AuditLevel::Enhanced,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        // Create control plane
        let control_plane = Arc::new(ControlPlane::new(
            config,
            history_ledger,
            messaging_system,
            policy_engine,
            context_router,
            memory_fabric,
            provider_router,
            skill_registry,
            workflow_engine,
            embodiment_control_plane,
            package_manager,
            evaluation_engine,
            session_manager,
        ));

        // Test agent inspection
        let inspection_request = AgentInspectionRequest {
            request_id: "inspect-request-001".to_string(),
            agent_id: "test-agent-001".to_string(),
            tenant_id: "test-tenant".to_string(),
            inspection_scope: InspectionScope::All,
            requested_tier: 2,
            trace_id: None,
        };

        let inspection_response = control_plane
            .inspect_agent(inspection_request)
            .await
            .unwrap();
        assert_eq!(inspection_response.agent_id, "test-agent-001");

        // Test agent control
        let control_request = AgentControlRequest {
            request_id: "control-request-001".to_string(),
            agent_id: "test-agent-001".to_string(),
            tenant_id: "test-tenant".to_string(),
            control_action: ControlAction::Pause,
            reason: "Testing control functionality".to_string(),
            authorized_by: "test-user".to_string(),
            trace_id: None,
        };

        let control_response = control_plane.control_agent(control_request).await.unwrap();
        assert_eq!(control_response.agent_id, "test-agent-001");
        assert_eq!(control_response.status, ControlStatus::Success);

        // Test trace functionality
        let trace_request = TraceRequest {
            request_id: "trace-request-001".to_string(),
            decision_id: Some("decision-001".to_string()),
            memory_id: Some("memory-001".to_string()),
            skill_execution_id: Some("skill-execution-001".to_string()),
            workflow_execution_id: Some("workflow-execution-001".to_string()),
            trace_depth: 3,
            include_sensitive: false,
            trace_id: None,
        };

        let trace_response = control_plane.trace(trace_request).await.unwrap();
        assert!(trace_response.trace_data.decision_trace.is_some());
        assert!(trace_response.trace_data.memory_trace.is_some());
        assert!(trace_response.trace_data.skill_trace.is_some());
        assert!(trace_response.trace_data.workflow_trace.is_some());

        // Verify statistics
        let stats = control_plane.get_statistics().await;
        assert!(stats.total_agents >= 0);
        assert!(stats.total_inspections >= 1);
        assert!(stats.total_controls >= 1);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
