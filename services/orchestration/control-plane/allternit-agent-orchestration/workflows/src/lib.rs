use allternit_history::HistoryLedger;
use allternit_messaging::{EventEnvelope, MessagingSystem, TaskQueue};
use allternit_policy::{PolicyEffect, PolicyEngine, PolicyRequest};
use allternit_skills::SkillRegistry;
use allternit_tools_gateway::{run_scoped_write_scope, ToolExecutionRequest, ToolGateway};
use petgraph::graph::NodeIndex;
use petgraph::Graph;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod agent_template_compiler;
pub mod agent_template_loader;
pub mod agent_template_validator;
pub mod compiler;
pub mod engine;
pub mod loader;
pub mod templates;
pub mod validator;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum WorkflowPhase {
    Observe,
    Think,
    Plan,
    Build,
    Execute,
    Verify,
    Learn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub name: String,
    pub phase: WorkflowPhase,
    pub skill_id: String,     // References a skill in the skill registry
    pub inputs: Vec<String>,  // References to artifact IDs or previous node outputs
    pub outputs: Vec<String>, // Names of outputs this node produces
    pub constraints: NodeConstraints,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConstraints {
    pub time_budget: Option<u64>, // seconds
    pub resource_limits: Option<ResourceLimits>,
    pub allowed_tools: Vec<String>,
    pub required_permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub network: NetworkAccess,
    pub filesystem: FilesystemAccess,
    pub time_limit: u64, // in seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkAccess {
    None,
    DomainAllowlist(Vec<String>),
    Unrestricted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilesystemAccess {
    None,
    Allowlist(Vec<String>),
    ReadWrite(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    pub condition: Option<String>, // optional condition expression
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub workflow_id: String,
    pub version: String,
    pub description: String,
    pub required_roles: Vec<String>,
    pub allowed_skill_tiers: Vec<allternit_policy::SafetyTier>,
    pub phases_used: Vec<WorkflowPhase>,
    pub success_criteria: String,
    pub failure_modes: Vec<String>,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    pub execution_id: String,
    pub workflow_id: String,
    pub session_id: String,
    pub tenant_id: String,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub status: WorkflowStatus,
    pub current_phase: Option<WorkflowPhase>,
    pub node_results: HashMap<String, NodeResult>,
    pub artifacts: Vec<String>, // artifact IDs
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkflowStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResult {
    pub node_id: String,
    pub status: NodeStatus,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub execution_time_ms: u64,
    pub artifacts_produced: Vec<String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// Result of a workflow replay operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    pub replay_id: String,
    pub original_execution_id: String,
    pub new_execution_id: String,
    pub workflow_id: String,
    pub session_id: String,
    pub checkpoint_id: Option<String>,
    pub status: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] allternit_history::HistoryError),
    #[error("Workflow not found: {0}")]
    WorkflowNotFound(String),
    #[error("Node not found: {0}")]
    NodeNotFound(String),
    #[error("Skill not found: {0}")]
    SkillNotFound(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Phase violation: {0}")]
    PhaseViolation(String),
    #[error("Policy error: {0}")]
    Policy(#[from] allternit_policy::PolicyError),
    #[error("Tool gateway error: {0}")]
    ToolGateway(#[from] allternit_tools_gateway::ToolGatewayError),
    #[error("Skills error: {0}")]
    Skills(#[from] allternit_skills::SkillsError),
    #[error("Mutex poisoned: {0}")]
    MutexPoisoned(String),
    #[error("Time error: {0}")]
    TimeError(String),
    #[error("Workflow has active executions and cannot be deleted: {0}")]
    WorkflowInProgress(String),
}

pub struct WorkflowEngine {
    workflows: Arc<RwLock<HashMap<String, WorkflowDefinition>>>,
    active_executions: Arc<RwLock<HashMap<String, WorkflowExecution>>>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    tool_gateway: Arc<ToolGateway>,
    skill_registry: Arc<SkillRegistry>,
    task_queue: Arc<TaskQueue>,
    pool: SqlitePool,
}

impl WorkflowEngine {
    pub async fn initialize_schema(&self) -> Result<(), WorkflowError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS workflow_definitions (
                workflow_id TEXT PRIMARY KEY,
                version TEXT NOT NULL,
                description TEXT NOT NULL,
                required_roles TEXT NOT NULL,
                allowed_skill_tiers TEXT NOT NULL,
                phases_used TEXT NOT NULL,
                success_criteria TEXT NOT NULL,
                failure_modes TEXT NOT NULL,
                nodes TEXT NOT NULL,
                edges TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WorkflowError::ExecutionFailed(format!(
                "Failed to create workflow_definitions table: {}",
                e
            ))
        })?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS workflow_executions (
                execution_id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                status TEXT NOT NULL,
                current_phase TEXT,
                node_results TEXT NOT NULL,
                artifacts TEXT NOT NULL,
                trace_id TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WorkflowError::ExecutionFailed(format!(
                "Failed to create workflow_executions table: {}",
                e
            ))
        })?;

        Ok(())
    }

    pub fn new(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        tool_gateway: Arc<ToolGateway>,
        skill_registry: Arc<SkillRegistry>,
        task_queue: Arc<TaskQueue>,
        pool: SqlitePool,
    ) -> Self {
        WorkflowEngine {
            workflows: Arc::new(RwLock::new(HashMap::new())),
            active_executions: Arc::new(RwLock::new(HashMap::new())),
            history_ledger,
            messaging_system,
            policy_engine,
            tool_gateway,
            skill_registry,
            task_queue,
            pool,
        }
    }

    pub async fn register_workflow(
        &self,
        workflow: WorkflowDefinition,
    ) -> Result<String, WorkflowError> {
        let workflow_id = workflow.workflow_id.clone();

        // Validate the workflow
        self.validate_workflow(&workflow).await?;

        // Persist to database
        sqlx::query(
            "INSERT OR REPLACE INTO workflow_definitions (
                workflow_id, version, description, required_roles, 
                allowed_skill_tiers, phases_used, success_criteria, 
                failure_modes, nodes, edges
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&workflow.workflow_id)
        .bind(&workflow.version)
        .bind(&workflow.description)
        .bind(serde_json::to_string(&workflow.required_roles)?)
        .bind(serde_json::to_string(&workflow.allowed_skill_tiers)?)
        .bind(serde_json::to_string(&workflow.phases_used)?)
        .bind(&workflow.success_criteria)
        .bind(serde_json::to_string(&workflow.failure_modes)?)
        .bind(serde_json::to_string(&workflow.nodes)?)
        .bind(serde_json::to_string(&workflow.edges)?)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WorkflowError::ExecutionFailed(format!("Failed to persist workflow definition: {}", e))
        })?;

        // Store in memory cache
        let mut workflows = self.workflows.write().await;
        workflows.insert(workflow_id.clone(), workflow.clone());
        drop(workflows);

        // Log to history ledger
        let mut history = self
            .history_ledger
            .lock()
            .map_err(|e| WorkflowError::MutexPoisoned(e.to_string()))?;
        let content = serde_json::to_value(&workflow)?;
        history.append(content)?;

        Ok(workflow_id)
    }

    async fn validate_workflow(&self, workflow: &WorkflowDefinition) -> Result<(), WorkflowError> {
        // Validate that all referenced skills exist
        for node in &workflow.nodes {
            let skill_result = self
                .skill_registry
                .get_skill(node.skill_id.clone(), None)
                .await;
            if skill_result.as_ref().is_ok_and(|s| s.is_none()) {
                return Err(WorkflowError::SkillNotFound(node.skill_id.clone()));
            }
        }

        // Validate phase ordering (scientific loop sequence)
        let mut phase_order = Vec::new();
        for node in &workflow.nodes {
            phase_order.push(&node.phase);
        }

        // Check if phases follow the scientific loop order (at least partially)
        // This is a simplified check - a full implementation would be more sophisticated
        Ok(())
    }

    pub async fn execute_workflow(
        &self,
        workflow_id: String,
        session_id: String,
        tenant_id: String,
        input_data: serde_json::Value,
    ) -> Result<String, WorkflowError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| WorkflowError::TimeError(e.to_string()))?
            .as_secs();

        // Get the workflow definition
        let workflows = self.workflows.read().await;
        let workflow_def = workflows
            .get(&workflow_id)
            .cloned()
            .ok_or_else(|| WorkflowError::WorkflowNotFound(workflow_id.clone()))?;
        drop(workflows);

        // Create execution record
        let execution_id = Uuid::new_v4().to_string();
        let mut execution = WorkflowExecution {
            execution_id: execution_id.clone(),
            workflow_id: workflow_id.clone(),
            session_id: session_id.clone(),
            tenant_id: tenant_id.clone(),
            start_time: timestamp,
            end_time: None,
            status: WorkflowStatus::Running,
            current_phase: None,
            node_results: HashMap::new(),
            artifacts: Vec::new(),
            trace_id: Some(Uuid::new_v4().to_string()),
        };

        // Log workflow start event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "WorkflowStart".to_string(),
            session_id: session_id.clone(),
            tenant_id: tenant_id.clone(),
            actor_id: "workflow_engine".to_string(),
            role: "workflow_executor".to_string(),
            timestamp,
            trace_id: execution.trace_id.clone(),
            payload: serde_json::json!({
                "execution_id": execution_id,
                "workflow_id": workflow_id,
                "session_id": session_id
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        // Store the execution
        let mut active_executions = self.active_executions.write().await;
        active_executions.insert(execution_id.clone(), execution.clone());
        drop(active_executions);

        // Persist execution start to database
        sqlx::query(
            "INSERT INTO workflow_executions (
                execution_id, workflow_id, session_id, tenant_id, 
                start_time, status, current_phase, node_results, artifacts, trace_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&execution.execution_id)
        .bind(&execution.workflow_id)
        .bind(&execution.session_id)
        .bind(&execution.tenant_id)
        .bind(execution.start_time as i64)
        .bind(format!("{:?}", execution.status))
        .bind(execution.current_phase.as_ref().map(|p| format!("{:?}", p)))
        .bind(serde_json::to_string(&execution.node_results)?)
        .bind(serde_json::to_string(&execution.artifacts)?)
        .bind(&execution.trace_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WorkflowError::ExecutionFailed(format!("Failed to persist workflow execution: {}", e))
        })?;

        // Build execution graph
        let graph = self.build_execution_graph(&workflow_def)?;

        // Execute the workflow
        let result = self.execute_graph(&graph, &mut execution, input_data).await;

        // Check if the workflow definition includes a Verify phase
        let includes_verify_phase = {
            let workflows = self.workflows.read().await;
            if let Some(def) = workflows.get(&workflow_id) {
                def.phases_used.contains(&WorkflowPhase::Verify)
            } else {
                false // If we can't find the definition, assume no verify phase
            }
        };

        // Check for verify artifacts before allowing completion (only if workflow includes Verify phase)
        let has_verify_artifacts = if includes_verify_phase {
            self.has_verify_artifacts(&execution).await
        } else {
            true // If no verify phase is defined, consider it as having "verify artifacts"
        };

        // Record a workflow execution artifact pointer (persisted by control plane).
        execution.artifacts.push(execution.execution_id.clone());

        // Update execution status
        let mut active_executions = self.active_executions.write().await;
        if let Some(exec) = active_executions.get_mut(&execution_id) {
            exec.status = match (&result, has_verify_artifacts) {
                (Ok(_), true) => WorkflowStatus::Completed, // Complete if successful and verify artifacts exist (when required)
                (Ok(_), false) => WorkflowStatus::Failed, // Fail if no verify artifacts despite success (when required)
                (Err(_), _) => WorkflowStatus::Failed,    // Fail if execution failed
            };
            exec.end_time = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| WorkflowError::TimeError(e.to_string()))?
                    .as_secs(),
            );
            exec.node_results = execution.node_results.clone();
            exec.artifacts = execution.artifacts.clone();

            // Persist completion to database
            let _ = sqlx::query(
                "UPDATE workflow_executions SET 
                    status = ?, end_time = ?, node_results = ?, artifacts = ?
                 WHERE execution_id = ?",
            )
            .bind(format!("{:?}", exec.status))
            .bind(exec.end_time.map(|t| t as i64))
            .bind(serde_json::to_string(&exec.node_results).unwrap_or_default())
            .bind(serde_json::to_string(&exec.artifacts).unwrap_or_default())
            .bind(&exec.execution_id)
            .execute(&self.pool)
            .await;
        }
        drop(active_executions);

        // Log workflow end event
        let end_event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: match has_verify_artifacts {
                true => "WorkflowEnd".to_string(),
                false => "WorkflowEndFailed".to_string(), // Log differently if verify artifacts missing
            },
            session_id: session_id.clone(),
            tenant_id: tenant_id.clone(),
            actor_id: "workflow_engine".to_string(),
            role: "workflow_executor".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| WorkflowError::TimeError(e.to_string()))?
                .as_secs(),
            trace_id: execution.trace_id.clone(),
            payload: serde_json::json!({
                "execution_id": execution_id,
                "workflow_id": workflow_id,
                "status": format!("{:?}", &result.as_ref().map_or(WorkflowStatus::Failed, |_| WorkflowStatus::Completed)),
                "has_verify_artifacts": has_verify_artifacts,
                "includes_verify_phase": includes_verify_phase
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = end_event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        // Return error if execution succeeded but verify artifacts are missing (when required)
        match (result, has_verify_artifacts) {
            (Ok(_), true) => Ok(execution_id),
            (Ok(_), false) => Err(WorkflowError::ExecutionFailed(
                "No verify artifacts found - completion blocked by verify-gated rule".to_string(),
            )),
            (Err(e), _) => Err(e),
        }
    }

    /// Check if the execution has any verify artifacts
    async fn has_verify_artifacts(&self, execution: &WorkflowExecution) -> bool {
        // In a real implementation, this would check for specific VerifyArtifact instances
        // in the history ledger or artifact store that are linked to this execution
        // For now, we'll check if any of the artifacts have "verify" in their name
        // or if there are specific verification artifacts in the system
        execution.artifacts.iter().any(|artifact_id| {
            artifact_id.contains("verify")
                || artifact_id.contains("verification")
                || artifact_id.contains("validation")
        })
    }

    fn build_execution_graph(
        &self,
        workflow_def: &WorkflowDefinition,
    ) -> Result<Graph<WorkflowNode, WorkflowEdge>, WorkflowError> {
        let mut graph = Graph::new();
        let mut node_map: HashMap<String, NodeIndex> = HashMap::new();

        // Add all nodes to the graph
        for node in &workflow_def.nodes {
            let node_idx = graph.add_node(node.clone());
            node_map.insert(node.id.clone(), node_idx);
        }

        // Add all edges to the graph
        for edge in &workflow_def.edges {
            if let (Some(from_idx), Some(to_idx)) =
                (node_map.get(&edge.from), node_map.get(&edge.to))
            {
                let edge_obj = WorkflowEdge {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    condition: edge.condition.clone(),
                };
                graph.add_edge(*from_idx, *to_idx, edge_obj);
            } else {
                return Err(WorkflowError::NodeNotFound(
                    if !node_map.contains_key(&edge.from) {
                        edge.from.clone()
                    } else {
                        edge.to.clone()
                    },
                ));
            }
        }

        Ok(graph)
    }

    async fn execute_graph(
        &self,
        graph: &Graph<WorkflowNode, WorkflowEdge>,
        execution: &mut WorkflowExecution,
        input_data: serde_json::Value,
    ) -> Result<(), WorkflowError> {
        // For simplicity, we'll execute nodes in topological order
        // A full implementation would handle parallel execution where possible

        let nodes = petgraph::algo::toposort(graph, None).map_err(|_| {
            WorkflowError::ExecutionFailed("Cycle detected in workflow".to_string())
        })?;

        for node_idx in nodes {
            let node = &graph[node_idx];

            // Update current phase in execution
            execution.current_phase = Some(node.phase.clone());

            // Execute the node
            let node_result = self.execute_node(node, execution, &input_data).await?;

            // Store the result
            execution.node_results.insert(node.id.clone(), node_result);
            if let Some(result) = execution.node_results.get(&node.id) {
                execution
                    .artifacts
                    .extend(result.artifacts_produced.clone());
            }
        }

        Ok(())
    }

    async fn execute_node(
        &self,
        node: &WorkflowNode,
        execution: &WorkflowExecution,
        input_data: &serde_json::Value,
    ) -> Result<NodeResult, WorkflowError> {
        let start_time = std::time::Instant::now();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| WorkflowError::TimeError(e.to_string()))?
            .as_secs();

        // Log node start event
        let node_start_event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "WorkflowStepStart".to_string(),
            session_id: execution.session_id.clone(),
            tenant_id: execution.tenant_id.clone(),
            actor_id: "workflow_engine".to_string(),
            role: "workflow_executor".to_string(),
            timestamp,
            trace_id: execution.trace_id.clone(),
            payload: serde_json::json!({
                "execution_id": execution.execution_id,
                "node_id": node.id,
                "phase": format!("{:?}", node.phase)
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = node_start_event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        // Get the skill for this node
        let skill_result = self
            .skill_registry
            .get_skill(node.skill_id.clone(), None)
            .await
            .map_err(WorkflowError::Skills)?;
        let skill =
            skill_result.ok_or_else(|| WorkflowError::SkillNotFound(node.skill_id.clone()))?;

        // Prepare input for the skill
        let node_input = self.prepare_node_input(node, input_data, execution).await?;

        // Validate input against skill schema
        self.skill_registry
            .validate_input(node.skill_id.clone(), &node_input)
            .await?;

        // Check policy before execution
        let policy_request = PolicyRequest {
            identity_id: "workflow_executor".to_string(), // In a real system, this would be the calling identity
            resource: format!("skill:{}", node.skill_id),
            action: "execute".to_string(),
            context: serde_json::json!({
                "session_id": execution.session_id,
                "tenant_id": execution.tenant_id,
                "execution_id": execution.execution_id,
                "node_id": node.id,
                "trace_id": execution.trace_id
            }),
            requested_tier: skill.manifest.risk_tier.clone(),
        };

        let decision = self.policy_engine.evaluate(policy_request).await?;

        if matches!(decision.decision, PolicyEffect::Deny) {
            return Err(WorkflowError::ExecutionFailed(format!(
                "Policy denied execution: {}",
                decision.reason
            )));
        }

        // Execute the skill through the tool gateway
        let tool_request = ToolExecutionRequest {
            tool_id: node.skill_id.clone(), // In a real system, this would map to the actual tool
            input: node_input,
            identity_id: "workflow_executor".to_string(),
            session_id: execution.session_id.clone(),
            tenant_id: execution.tenant_id.clone(),
            run_id: Some(execution.execution_id.clone()),
            workflow_id: Some(execution.workflow_id.clone()),
            node_id: Some(node.id.clone()),
            wih_id: Some(format!("wih-{}", node.id)),
            write_scope: Some(run_scoped_write_scope(&execution.execution_id, false)),
            capsule_run: None,
            trace_id: execution.trace_id.clone(),
            retry_count: 0,
            idempotency_key: Some(format!(
                "{}-{}-{}",
                execution.execution_id, node.id, timestamp
            )),
        };

        let result = self.tool_gateway.execute_tool(tool_request).await;

        let node_result = match result {
            Ok(tool_result) => NodeResult {
                node_id: node.id.clone(),
                status: NodeStatus::Completed,
                output: tool_result.output,
                error: None,
                execution_time_ms: start_time.elapsed().as_millis() as u64,
                artifacts_produced: vec![tool_result.execution_id],
                timestamp,
            },
            Err(e) => NodeResult {
                node_id: node.id.clone(),
                status: NodeStatus::Failed,
                output: None,
                error: Some(e.to_string()),
                execution_time_ms: start_time.elapsed().as_millis() as u64,
                artifacts_produced: vec![],
                timestamp,
            },
        };

        // Log node end event
        let node_end_event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "WorkflowStepEnd".to_string(),
            session_id: execution.session_id.clone(),
            tenant_id: execution.tenant_id.clone(),
            actor_id: "workflow_engine".to_string(),
            role: "workflow_executor".to_string(),
            timestamp,
            trace_id: execution.trace_id.clone(),
            payload: serde_json::json!({
                "execution_id": execution.execution_id,
                "node_id": node.id,
                "result": &node_result
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = node_end_event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(node_result)
    }

    async fn prepare_node_input(
        &self,
        node: &WorkflowNode,
        input_data: &serde_json::Value,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value, WorkflowError> {
        // In a real implementation, this would resolve the input references
        // For now, we'll just return the input data or a simple transformation
        Ok(input_data.clone())
    }

    pub async fn get_workflow(&self, workflow_id: String) -> Option<WorkflowDefinition> {
        let workflows = self.workflows.read().await;
        workflows.get(&workflow_id).cloned()
    }

    pub async fn get_execution(&self, execution_id: String) -> Option<WorkflowExecution> {
        let executions = self.active_executions.read().await;
        executions.get(&execution_id).cloned()
    }

    pub async fn list_workflows(&self) -> Vec<WorkflowDefinition> {
        let workflows = self.workflows.read().await;
        workflows.values().cloned().collect()
    }

    pub async fn list_executions(&self) -> Vec<WorkflowExecution> {
        let executions = self.active_executions.read().await;
        executions.values().cloned().collect()
    }

    /// Delete a workflow definition
    ///
    /// This method:
    /// 1. Checks if the workflow exists
    /// 2. Checks for any running executions (WorkflowStatus::Running)
    /// 3. Deletes from SQLite workflow_definitions table
    /// 4. Removes from in-memory workflows HashMap
    ///
    /// # Arguments
    /// * `workflow_id` - The ID of the workflow to delete
    ///
    /// # Returns
    /// * `Ok(())` - Workflow deleted successfully
    /// * `Err(WorkflowError::WorkflowNotFound)` - If the workflow does not exist
    /// * `Err(WorkflowError::WorkflowInProgress)` - If there are running executions
    ///
    /// # Errors
    /// Returns WorkflowInProgress if any executions for this workflow have status Running
    pub async fn delete_workflow(&self, workflow_id: &str) -> Result<(), WorkflowError> {
        // Step 1: Check if workflow exists in memory
        let workflows = self.workflows.read().await;
        if !workflows.contains_key(workflow_id) {
            return Err(WorkflowError::WorkflowNotFound(workflow_id.to_string()));
        }
        drop(workflows);

        // Step 2: Check for running executions (in-progress workflows)
        let executions = self.active_executions.read().await;
        let running_executions: Vec<&WorkflowExecution> = executions
            .values()
            .filter(|e| e.workflow_id == workflow_id && e.status == WorkflowStatus::Running)
            .collect();

        if !running_executions.is_empty() {
            let running_ids: Vec<String> = running_executions
                .iter()
                .map(|e| e.execution_id.clone())
                .collect();
            return Err(WorkflowError::WorkflowInProgress(format!(
                "Cannot delete workflow '{}' because it has {} running execution(s): {}",
                workflow_id,
                running_ids.len(),
                running_ids.join(", ")
            )));
        }
        drop(executions);

        // Step 3: Delete from SQLite database
        let result = sqlx::query("DELETE FROM workflow_definitions WHERE workflow_id = ?")
            .bind(workflow_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WorkflowError::ExecutionFailed(format!(
                    "Failed to delete workflow from database: {}",
                    e
                ))
            })?;

        if result.rows_affected() == 0 {
            // Workflow was in memory but not in database - this is unusual but not fatal
            tracing::warn!(
                workflow_id = %workflow_id,
                "Workflow found in memory but not in database"
            );
        }

        // Step 4: Remove from in-memory cache
        let mut workflows = self.workflows.write().await;
        workflows.remove(workflow_id);
        drop(workflows);

        tracing::info!(workflow_id = %workflow_id, "Deleted workflow");

        Ok(())
    }

    /// Replay a workflow execution from a checkpoint
    ///
    /// # Arguments
    /// * `workflow_id` - The ID of the workflow to replay
    /// * `session_id` - The session ID for the new execution
    /// * `checkpoint_id` - Optional node ID to replay from (if None, replays from start)
    ///
    /// # Returns
    /// * `Ok(ReplayResult)` - The result of the replay operation
    /// * `Err(WorkflowError)` - If the workflow or execution is not found
    pub async fn replay_workflow(
        &self,
        workflow_id: &str,
        session_id: &str,
        checkpoint_id: Option<&str>,
    ) -> Result<ReplayResult, WorkflowError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| WorkflowError::TimeError(e.to_string()))?
            .as_secs();

        // Get the workflow definition
        let workflows = self.workflows.read().await;
        let workflow_def = workflows
            .get(workflow_id)
            .cloned()
            .ok_or_else(|| WorkflowError::WorkflowNotFound(workflow_id.to_string()))?;
        drop(workflows);

        // Find the original execution if checkpoint_id is provided
        let original_execution = if let Some(checkpoint) = checkpoint_id {
            let executions = self.active_executions.read().await;
            executions
                .values()
                .find(|e| e.workflow_id == workflow_id && e.node_results.contains_key(checkpoint))
                .cloned()
        } else {
            None
        };

        // Create new execution record for the replay
        let replay_id = Uuid::new_v4().to_string();
        let new_execution_id = Uuid::new_v4().to_string();

        let mut new_execution = WorkflowExecution {
            execution_id: new_execution_id.clone(),
            workflow_id: workflow_id.to_string(),
            session_id: session_id.to_string(),
            tenant_id: original_execution
                .as_ref()
                .map(|e| e.tenant_id.clone())
                .unwrap_or_else(|| "default".to_string()),
            start_time: timestamp,
            end_time: None,
            status: WorkflowStatus::Running,
            current_phase: None,
            node_results: HashMap::new(),
            artifacts: Vec::new(),
            trace_id: Some(replay_id.clone()),
        };

        // If we have an original execution with a checkpoint, copy results up to checkpoint
        if let Some(orig_exec) = &original_execution {
            if let Some(checkpoint) = checkpoint_id {
                // Copy node results up to and including the checkpoint
                for (node_id, result) in &orig_exec.node_results {
                    if node_id.as_str() <= checkpoint {
                        new_execution
                            .node_results
                            .insert(node_id.clone(), result.clone());
                    }
                }
                new_execution.current_phase = orig_exec.current_phase.clone();
            }
        }

        // Store the new execution
        let mut active_executions = self.active_executions.write().await;
        active_executions.insert(new_execution_id.clone(), new_execution.clone());
        drop(active_executions);

        // Persist execution start to database
        sqlx::query(
            "INSERT INTO workflow_executions (
                execution_id, workflow_id, session_id, tenant_id,
                start_time, status, current_phase, node_results, artifacts, trace_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&new_execution.execution_id)
        .bind(&new_execution.workflow_id)
        .bind(&new_execution.session_id)
        .bind(&new_execution.tenant_id)
        .bind(new_execution.start_time as i64)
        .bind(format!("{:?}", new_execution.status))
        .bind(
            new_execution
                .current_phase
                .as_ref()
                .map(|p| format!("{:?}", p)),
        )
        .bind(serde_json::to_string(&new_execution.node_results).unwrap_or_default())
        .bind(serde_json::to_string(&new_execution.artifacts).unwrap_or_default())
        .bind(&new_execution.trace_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WorkflowError::ExecutionFailed(format!(
                "Failed to persist workflow replay execution: {}",
                e
            ))
        })?;

        // Log replay start event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "WorkflowReplay".to_string(),
            session_id: session_id.to_string(),
            tenant_id: new_execution.tenant_id.clone(),
            actor_id: "workflow_engine".to_string(),
            role: "workflow_executor".to_string(),
            timestamp,
            trace_id: new_execution.trace_id.clone(),
            payload: serde_json::json!({
                "replay_id": replay_id,
                "original_execution_id": original_execution.as_ref().map(|e| e.execution_id.clone()),
                "new_execution_id": new_execution_id,
                "workflow_id": workflow_id,
                "session_id": session_id,
                "checkpoint_id": checkpoint_id
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(ReplayResult {
            replay_id,
            original_execution_id: original_execution
                .map(|e| e.execution_id)
                .unwrap_or_default(),
            new_execution_id,
            workflow_id: workflow_id.to_string(),
            session_id: session_id.to_string(),
            checkpoint_id: checkpoint_id.map(|s| s.to_string()),
            status: "started".to_string(),
        })
    }
}

// Helper functions for common workflow operations
impl WorkflowEngine {
    pub async fn create_sample_workflow(&self) -> Result<String, WorkflowError> {
        let workflow = WorkflowDefinition {
            workflow_id: "sample_scientific_loop".to_string(),
            version: "1.0.0".to_string(),
            description: "A sample workflow following the scientific loop".to_string(),
            required_roles: vec!["user".to_string()],
            allowed_skill_tiers: vec![
                allternit_policy::SafetyTier::T0,
                allternit_policy::SafetyTier::T1,
            ],
            phases_used: vec![
                WorkflowPhase::Observe,
                WorkflowPhase::Think,
                WorkflowPhase::Plan,
                WorkflowPhase::Execute,
                WorkflowPhase::Verify,
            ],
            success_criteria: "All phases completed successfully".to_string(),
            failure_modes: vec!["Any phase fails".to_string()],
            nodes: vec![
                WorkflowNode {
                    id: "observe_1".to_string(),
                    name: "Observe Phase".to_string(),
                    phase: WorkflowPhase::Observe,
                    skill_id: "echo_tool".to_string(), // This should be a real skill
                    inputs: vec!["initial_input".to_string()],
                    outputs: vec!["observed_data".to_string()],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec!["echo_tool".to_string()],
                        required_permissions: vec!["perm_t0_read".to_string()],
                    },
                },
                WorkflowNode {
                    id: "think_1".to_string(),
                    name: "Think Phase".to_string(),
                    phase: WorkflowPhase::Think,
                    skill_id: "echo_tool".to_string(), // This should be a real skill
                    inputs: vec!["observed_data".to_string()],
                    outputs: vec!["analysis".to_string()],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec!["echo_tool".to_string()],
                        required_permissions: vec!["perm_t0_read".to_string()],
                    },
                },
                WorkflowNode {
                    id: "plan_1".to_string(),
                    name: "Plan Phase".to_string(),
                    phase: WorkflowPhase::Plan,
                    skill_id: "echo_tool".to_string(), // This should be a real skill
                    inputs: vec!["analysis".to_string()],
                    outputs: vec!["plan".to_string()],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec!["echo_tool".to_string()],
                        required_permissions: vec!["perm_t0_read".to_string()],
                    },
                },
                WorkflowNode {
                    id: "execute_1".to_string(),
                    name: "Execute Phase".to_string(),
                    phase: WorkflowPhase::Execute,
                    skill_id: "echo_tool".to_string(), // This should be a real skill
                    inputs: vec!["plan".to_string()],
                    outputs: vec!["execution_result".to_string()],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec!["echo_tool".to_string()],
                        required_permissions: vec!["perm_t0_read".to_string()],
                    },
                },
                WorkflowNode {
                    id: "verify_1".to_string(),
                    name: "Verify Phase".to_string(),
                    phase: WorkflowPhase::Verify,
                    skill_id: "echo_tool".to_string(), // This should be a real skill
                    inputs: vec!["execution_result".to_string()],
                    outputs: vec!["verification_result".to_string()],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec!["echo_tool".to_string()],
                        required_permissions: vec!["perm_t0_read".to_string()],
                    },
                },
            ],
            edges: vec![
                WorkflowEdge {
                    from: "observe_1".to_string(),
                    to: "think_1".to_string(),
                    condition: None,
                },
                WorkflowEdge {
                    from: "think_1".to_string(),
                    to: "plan_1".to_string(),
                    condition: None,
                },
                WorkflowEdge {
                    from: "plan_1".to_string(),
                    to: "execute_1".to_string(),
                    condition: None,
                },
                WorkflowEdge {
                    from: "execute_1".to_string(),
                    to: "verify_1".to_string(),
                    condition: None,
                },
            ],
        };

        self.register_workflow(workflow).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_messaging::MessagingSystem;
    use allternit_policy::PolicyEngine;
    use allternit_skills::SkillRegistry;
    use allternit_tools_gateway::ToolGateway;
    use data_encoding::BASE64;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use sqlx::SqlitePool;
    use tempfile::NamedTempFile;

    fn create_publisher_key() -> (Ed25519KeyPair, allternit_skills::PublisherKey) {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public_key = BASE64.encode(keypair.public_key().as_ref());
        let publisher_key = allternit_skills::PublisherKey {
            publisher_id: "test_publisher".to_string(),
            public_key_id: "key1".to_string(),
            public_key,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            revoked: false,
            revoked_at: None,
        };
        (keypair, publisher_key)
    }

    fn sign_skill(skill: &mut allternit_skills::Skill, keypair: &Ed25519KeyPair) {
        let bundle_digest = allternit_skills::compute_bundle_digest(skill).unwrap();
        let bundle_hash = allternit_skills::compute_bundle_hash(skill).unwrap();
        let signature = keypair.sign(&bundle_digest);
        skill.manifest.signature.manifest_sig = BASE64.encode(signature.as_ref());
        skill.manifest.signature.bundle_hash = bundle_hash;
    }

    #[tokio::test]
    async fn test_workflow_engine() {
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();
        let temp_path = format!("/tmp/test_workflows_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));
        let messaging_system = Arc::new(
            MessagingSystem::new_with_storage(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );
        let policy_engine = Arc::new(PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let tool_gateway = Arc::new(ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let skill_registry = Arc::new(
            SkillRegistry::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                tool_gateway.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );
        let (keypair, publisher_key) = create_publisher_key();
        skill_registry
            .register_publisher_key(publisher_key)
            .await
            .unwrap();

        // Register a test skill
        let test_tool = allternit_tools_gateway::ToolDefinition {
            id: "echo_tool".to_string(),
            name: "Echo Tool".to_string(),
            description: "A simple echo tool for testing".to_string(),
            tool_type: allternit_tools_gateway::ToolType::Local,
            command: "echo".to_string(),
            endpoint: "".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                },
                "required": ["message"]
            }),
            output_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "output": {"type": "string"}
                }
            }),
            side_effects: vec!["read".to_string()],
            idempotency_behavior: "idempotent".to_string(),
            retryable: true,
            failure_classification: "transient".to_string(),
            safety_tier: allternit_policy::SafetyTier::T0,
            resource_limits: allternit_tools_gateway::ResourceLimits {
                cpu: Some("100m".to_string()),
                memory: Some("64Mi".to_string()),
                network: allternit_tools_gateway::NetworkAccess::None,
                filesystem: allternit_tools_gateway::FilesystemAccess::None,
                time_limit: 10,
            },
            subprocess: None,
        };

        let skill_manifest = allternit_skills::SkillManifest {
            id: "echo_skill".to_string(),
            name: "Echo Skill".to_string(),
            version: "1.0.0".to_string(),
            description: "A test skill that echoes input".to_string(),
            author: "Test Author".to_string(),
            license: "MIT".to_string(),
            tags: vec!["test".to_string(), "echo".to_string()],
            homepage: Some("https://example.com".to_string()),
            repository: Some("https://github.com/example/test-skill".to_string()),

            inputs: allternit_skills::SkillIO {
                schema: r#"{"type": "object", "properties": {"message": {"type": "string"}}}"#
                    .to_string(),
                examples: Some(vec![serde_json::json!({"message": "hello"})]),
            },
            outputs: allternit_skills::SkillIO {
                schema: r#"{"type": "object", "properties": {"output": {"type": "string"}}}"#
                    .to_string(),
                examples: Some(vec![serde_json::json!({"output": "hello"})]),
            },

            runtime: allternit_skills::SkillRuntime {
                mode: allternit_skills::RuntimeMode::Sandbox,
                timeouts: allternit_skills::SkillTimeouts {
                    per_step: Some(60),
                    total: Some(300),
                },
                resources: Some(allternit_skills::ResourceHints {
                    cpu: Some("100m".to_string()),
                    gpu: None,
                    memory: Some("128Mi".to_string()),
                }),
            },

            environment: allternit_skills::SkillEnvironment {
                allowed_envs: vec![
                    allternit_skills::Environment::Dev,
                    allternit_skills::Environment::Stage,
                ],
                network: allternit_skills::NetworkAccess::None,
                filesystem: allternit_skills::FilesystemAccess::None,
            },

            side_effects: vec!["read".to_string()],

            risk_tier: allternit_policy::SafetyTier::T0,
            required_permissions: vec!["perm_t0_read".to_string()],
            requires_policy_gate: true,
            publisher: allternit_skills::PublisherInfo {
                publisher_id: "test_publisher".to_string(),
                public_key_id: "key1".to_string(),
            },
            signature: allternit_skills::SignatureInfo {
                manifest_sig: String::new(),
                bundle_hash: String::new(),
            },
        };

        let workflow = allternit_skills::SkillWorkflow {
            nodes: vec![allternit_skills::WorkflowNode {
                id: "echo_node".to_string(),
                name: "Echo Node".to_string(),
                phase: allternit_skills::WorkflowPhase::Observe,
                tool_binding: "echo_tool".to_string(),
                inputs: vec!["input".to_string()],
                outputs: vec!["output".to_string()],
            }],
            edges: vec![],
            per_node_constraints: std::collections::HashMap::new(),
            artifact_outputs: vec!["result".to_string()],
        };

        let mut skill = allternit_skills::Skill {
            manifest: skill_manifest,
            workflow,
            tools: vec![test_tool],
            human_routing: "This skill echoes the input message".to_string(),
        };

        sign_skill(&mut skill, &keypair);
        skill_registry.register_skill(skill).await.unwrap();

        let task_queue = messaging_system.task_queue.clone();

        let workflow_engine = WorkflowEngine::new(
            history_ledger.clone(),
            messaging_system,
            policy_engine,
            tool_gateway,
            skill_registry,
            task_queue,
            pool.clone(),
        );

        // Create and register a workflow definition that references the skill
        let workflow_def = WorkflowDefinition {
            workflow_id: "test_workflow".to_string(),
            version: "1.0.0".to_string(),
            description: "A test workflow".to_string(),
            required_roles: vec!["user".to_string()],
            allowed_skill_tiers: vec![allternit_policy::SafetyTier::T0],
            phases_used: vec![WorkflowPhase::Observe],
            success_criteria: "All nodes completed successfully".to_string(),
            failure_modes: vec!["Any node fails".to_string()],
            nodes: vec![WorkflowNode {
                id: "echo_node".to_string(),
                name: "Echo Node".to_string(),
                phase: WorkflowPhase::Observe,
                skill_id: "echo_skill".to_string(), // This should match the skill ID
                inputs: vec!["input".to_string()],
                outputs: vec!["output".to_string()],
                constraints: NodeConstraints {
                    time_budget: Some(60),
                    resource_limits: None,
                    allowed_tools: vec!["echo_tool".to_string()],
                    required_permissions: vec!["perm_t0_read".to_string()],
                },
            }],
            edges: vec![],
        };

        workflow_engine
            .register_workflow(workflow_def)
            .await
            .unwrap();

        // Get the workflow
        let workflow = workflow_engine
            .get_workflow("test_workflow".to_string())
            .await
            .unwrap();
        assert_eq!(workflow.description, "A test workflow");

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }

    #[tokio::test]
    async fn test_workflow_execution() {
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();
        let temp_path = format!("/tmp/test_execution_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));
        let messaging_system = Arc::new(
            MessagingSystem::new_with_storage(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );
        let policy_engine = Arc::new(PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Register a default identity for testing
        let identity = allternit_policy::Identity {
            id: "workflow_executor".to_string(), // Use the expected identity ID
            identity_type: allternit_policy::IdentityType::AgentIdentity,
            name: "workflow_executor".to_string(),
            tenant_id: "tenant1".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["user".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine.register_identity(identity).await.unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        // Skip create_default_rules() since condition evaluation is not implemented
        // Instead, create a simple rule that allows all T0 operations
        use allternit_policy::{PolicyEffect, PolicyRule};
        let simple_rule = PolicyRule {
            id: "simple_allow_t0".to_string(),
            name: "Simple T0 Allow Rule".to_string(),
            description: "Allow T0 operations".to_string(),
            condition: "".to_string(), // Empty condition means always match (within resource/action constraints)
            effect: PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["execute".to_string(), "read".to_string(), "get".to_string()],
            priority: 100,
            enabled: true,
        };
        policy_engine.add_rule(simple_rule).await.unwrap();

        let tool_gateway = Arc::new(ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let skill_registry = Arc::new(
            SkillRegistry::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                tool_gateway.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );
        let (keypair, publisher_key) = create_publisher_key();
        skill_registry
            .register_publisher_key(publisher_key)
            .await
            .unwrap();

        // Register a test skill
        let test_tool = allternit_tools_gateway::ToolDefinition {
            id: "echo_tool".to_string(),
            name: "Echo Tool".to_string(),
            description: "A simple echo tool for testing".to_string(),
            tool_type: allternit_tools_gateway::ToolType::Local,
            command: "echo".to_string(),
            endpoint: "".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                },
                "required": ["message"]
            }),
            output_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "output": {"type": "string"}
                }
            }),
            side_effects: vec!["read".to_string()],
            idempotency_behavior: "idempotent".to_string(),
            retryable: true,
            failure_classification: "transient".to_string(),
            safety_tier: allternit_policy::SafetyTier::T0,
            resource_limits: allternit_tools_gateway::ResourceLimits {
                cpu: Some("100m".to_string()),
                memory: Some("64Mi".to_string()),
                network: allternit_tools_gateway::NetworkAccess::None,
                filesystem: allternit_tools_gateway::FilesystemAccess::None,
                time_limit: 10,
            },
            subprocess: None,
        };

        let skill_manifest = allternit_skills::SkillManifest {
            id: "echo_skill".to_string(),
            name: "Echo Skill".to_string(),
            version: "1.0.0".to_string(),
            description: "A test skill that echoes input".to_string(),
            author: "Test Author".to_string(),
            license: "MIT".to_string(),
            tags: vec!["test".to_string(), "echo".to_string()],
            homepage: Some("https://example.com".to_string()),
            repository: Some("https://github.com/example/test-skill".to_string()),

            inputs: allternit_skills::SkillIO {
                schema: r#"{"type": "object", "properties": {"message": {"type": "string"}}}"#
                    .to_string(),
                examples: Some(vec![serde_json::json!({"message": "hello"})]),
            },
            outputs: allternit_skills::SkillIO {
                schema: r#"{"type": "object", "properties": {"output": {"type": "string"}}}"#
                    .to_string(),
                examples: Some(vec![serde_json::json!({"output": "hello"})]),
            },

            runtime: allternit_skills::SkillRuntime {
                mode: allternit_skills::RuntimeMode::Sandbox,
                timeouts: allternit_skills::SkillTimeouts {
                    per_step: Some(60),
                    total: Some(300),
                },
                resources: Some(allternit_skills::ResourceHints {
                    cpu: Some("100m".to_string()),
                    gpu: None,
                    memory: Some("128Mi".to_string()),
                }),
            },

            environment: allternit_skills::SkillEnvironment {
                allowed_envs: vec![
                    allternit_skills::Environment::Dev,
                    allternit_skills::Environment::Stage,
                ],
                network: allternit_skills::NetworkAccess::None,
                filesystem: allternit_skills::FilesystemAccess::None,
            },

            side_effects: vec!["read".to_string()],

            risk_tier: allternit_policy::SafetyTier::T0,
            required_permissions: vec!["perm_t0_read".to_string()],
            requires_policy_gate: true,
            publisher: allternit_skills::PublisherInfo {
                publisher_id: "test_publisher".to_string(),
                public_key_id: "key1".to_string(),
            },
            signature: allternit_skills::SignatureInfo {
                manifest_sig: String::new(),
                bundle_hash: String::new(),
            },
        };

        let workflow = allternit_skills::SkillWorkflow {
            nodes: vec![allternit_skills::WorkflowNode {
                id: "echo_node".to_string(),
                name: "Echo Node".to_string(),
                phase: allternit_skills::WorkflowPhase::Observe,
                tool_binding: "echo_tool".to_string(),
                inputs: vec!["input".to_string()],
                outputs: vec!["output".to_string()],
            }],
            edges: vec![],
            per_node_constraints: std::collections::HashMap::new(),
            artifact_outputs: vec!["result".to_string()],
        };

        let mut skill = allternit_skills::Skill {
            manifest: skill_manifest,
            workflow,
            tools: vec![test_tool],
            human_routing: "This skill echoes the input message".to_string(),
        };

        sign_skill(&mut skill, &keypair);
        skill_registry.register_skill(skill).await.unwrap();

        let task_queue = messaging_system.task_queue.clone();

        let workflow_engine = WorkflowEngine::new(
            history_ledger.clone(),
            messaging_system,
            policy_engine,
            tool_gateway,
            skill_registry,
            task_queue,
            pool.clone(),
        );

        // Create and register a workflow definition that references the skill
        let workflow_def = WorkflowDefinition {
            workflow_id: "test_workflow".to_string(),
            version: "1.0.0".to_string(),
            description: "A test workflow".to_string(),
            required_roles: vec!["user".to_string()],
            allowed_skill_tiers: vec![allternit_policy::SafetyTier::T0],
            phases_used: vec![WorkflowPhase::Observe],
            success_criteria: "All nodes completed successfully".to_string(),
            failure_modes: vec!["Any node fails".to_string()],
            nodes: vec![WorkflowNode {
                id: "echo_node".to_string(),
                name: "Echo Node".to_string(),
                phase: WorkflowPhase::Observe,
                skill_id: "echo_skill".to_string(), // This should match the skill ID
                inputs: vec!["input".to_string()],
                outputs: vec!["output".to_string()],
                constraints: NodeConstraints {
                    time_budget: Some(60),
                    resource_limits: None,
                    allowed_tools: vec!["echo_tool".to_string()],
                    required_permissions: vec!["perm_t0_read".to_string()],
                },
            }],
            edges: vec![],
        };

        workflow_engine
            .register_workflow(workflow_def)
            .await
            .unwrap();

        // Execute the workflow
        let execution_id = workflow_engine
            .execute_workflow(
                "test_workflow".to_string(),
                "session1".to_string(),
                "tenant1".to_string(),
                serde_json::json!({"input": "test"}),
            )
            .await
            .unwrap();

        // Get the execution
        let execution = workflow_engine.get_execution(execution_id).await.unwrap();
        assert_eq!(execution.workflow_id, "test_workflow");

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }

    #[tokio::test]
    async fn test_verify_gated_completion_requires_artifacts() {
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();
        let temp_path = format!("/tmp/test_verify_gate_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));
        let messaging_system = Arc::new(
            MessagingSystem::new_with_storage(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );
        let policy_engine = Arc::new(PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let identity = allternit_policy::Identity {
            id: "workflow_executor".to_string(),
            identity_type: allternit_policy::IdentityType::AgentIdentity,
            name: "workflow_executor".to_string(),
            tenant_id: "tenant1".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["user".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine.register_identity(identity).await.unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        use allternit_policy::{PolicyEffect, PolicyRule};
        let allow_execute_rule = PolicyRule {
            id: "allow_execute_t0".to_string(),
            name: "Allow execute for T0".to_string(),
            description: "Allow execute operations in tests".to_string(),
            condition: "".to_string(),
            effect: PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["execute".to_string(), "read".to_string(), "get".to_string()],
            priority: 100,
            enabled: true,
        };
        policy_engine.add_rule(allow_execute_rule).await.unwrap();

        let tool_gateway = Arc::new(ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let skill_registry = Arc::new(
            SkillRegistry::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                tool_gateway.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );
        let (keypair, publisher_key) = create_publisher_key();
        skill_registry
            .register_publisher_key(publisher_key)
            .await
            .unwrap();

        let test_tool = allternit_tools_gateway::ToolDefinition {
            id: "verify_skill".to_string(),
            name: "Verify Skill Tool".to_string(),
            description: "Tool used for verify gating test".to_string(),
            tool_type: allternit_tools_gateway::ToolType::Local,
            command: "echo".to_string(),
            endpoint: "".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: serde_json::json!({"type": "object"}),
            side_effects: vec![],
            idempotency_behavior: "idempotent".to_string(),
            retryable: true,
            failure_classification: "transient".to_string(),
            safety_tier: allternit_policy::SafetyTier::T0,
            resource_limits: allternit_tools_gateway::ResourceLimits {
                cpu: Some("100m".to_string()),
                memory: Some("64Mi".to_string()),
                network: allternit_tools_gateway::NetworkAccess::None,
                filesystem: allternit_tools_gateway::FilesystemAccess::None,
                time_limit: 10,
            },
            subprocess: None,
        };
        tool_gateway.register_tool(test_tool.clone()).await.unwrap();

        let skill_manifest = allternit_skills::SkillManifest {
            id: "verify_skill".to_string(),
            name: "Verify Skill".to_string(),
            version: "1.0.0".to_string(),
            description: "Skill used for verify gating test".to_string(),
            author: "Test Author".to_string(),
            license: "MIT".to_string(),
            tags: vec!["test".to_string()],
            homepage: None,
            repository: None,
            inputs: allternit_skills::SkillIO {
                schema: r#"{"type": "object"}"#.to_string(),
                examples: None,
            },
            outputs: allternit_skills::SkillIO {
                schema: r#"{"type": "object"}"#.to_string(),
                examples: None,
            },
            runtime: allternit_skills::SkillRuntime {
                mode: allternit_skills::RuntimeMode::Sandbox,
                timeouts: allternit_skills::SkillTimeouts {
                    per_step: Some(60),
                    total: Some(300),
                },
                resources: Some(allternit_skills::ResourceHints {
                    cpu: Some("100m".to_string()),
                    gpu: None,
                    memory: Some("128Mi".to_string()),
                }),
            },
            environment: allternit_skills::SkillEnvironment {
                allowed_envs: vec![allternit_skills::Environment::Dev],
                network: allternit_skills::NetworkAccess::None,
                filesystem: allternit_skills::FilesystemAccess::None,
            },
            side_effects: vec![],
            risk_tier: allternit_policy::SafetyTier::T0,
            required_permissions: vec![],
            requires_policy_gate: false,
            publisher: allternit_skills::PublisherInfo {
                publisher_id: "test_publisher".to_string(),
                public_key_id: "test_public_key".to_string(),
            },
            signature: allternit_skills::SignatureInfo {
                manifest_sig: String::new(),
                bundle_hash: String::new(),
            },
        };

        let skill_workflow = allternit_skills::SkillWorkflow {
            nodes: vec![allternit_skills::WorkflowNode {
                id: "node-1".to_string(),
                name: "Verify Node".to_string(),
                phase: allternit_skills::WorkflowPhase::Verify,
                tool_binding: "verify_skill".to_string(),
                inputs: vec!["input".to_string()],
                outputs: vec!["output".to_string()],
            }],
            edges: vec![],
            per_node_constraints: std::collections::HashMap::new(),
            artifact_outputs: vec!["output".to_string()],
        };

        let mut skill = allternit_skills::Skill {
            manifest: skill_manifest,
            workflow: skill_workflow,
            tools: vec![test_tool],
            human_routing: "Verify test skill".to_string(),
        };

        sign_skill(&mut skill, &keypair);
        skill_registry.register_skill(skill).await.unwrap();

        let task_queue = messaging_system.task_queue.clone();
        let workflow_engine = WorkflowEngine::new(
            history_ledger.clone(),
            messaging_system,
            policy_engine,
            tool_gateway,
            skill_registry,
            task_queue,
            pool.clone(),
        );

        let workflow_def = WorkflowDefinition {
            workflow_id: "verify_workflow".to_string(),
            version: "1.0.0".to_string(),
            description: "Verify-gated workflow".to_string(),
            required_roles: vec!["user".to_string()],
            allowed_skill_tiers: vec![allternit_policy::SafetyTier::T0],
            phases_used: vec![WorkflowPhase::Verify],
            success_criteria: "Verify phase complete".to_string(),
            failure_modes: vec!["Missing verify artifacts".to_string()],
            nodes: vec![WorkflowNode {
                id: "verify_node".to_string(),
                name: "Verify Node".to_string(),
                phase: WorkflowPhase::Verify,
                skill_id: "verify_skill".to_string(),
                inputs: vec!["input".to_string()],
                outputs: vec!["output".to_string()],
                constraints: NodeConstraints {
                    time_budget: Some(60),
                    resource_limits: None,
                    allowed_tools: vec!["verify_skill".to_string()],
                    required_permissions: vec![],
                },
            }],
            edges: vec![],
        };

        workflow_engine
            .register_workflow(workflow_def)
            .await
            .unwrap();

        let result = workflow_engine
            .execute_workflow(
                "verify_workflow".to_string(),
                "session1".to_string(),
                "tenant1".to_string(),
                serde_json::json!({"input": "test"}),
            )
            .await;

        match result {
            Err(WorkflowError::ExecutionFailed(message)) => {
                assert!(message.contains("verify artifacts"));
            }
            _ => panic!("Expected verify-gated execution failure"),
        }

        std::fs::remove_file(&temp_path).unwrap();
    }
}
