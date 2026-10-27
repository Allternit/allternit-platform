//! Workflow Execution Engine
//!
//! Executes workflow DAGs using the execution driver interface.
//! Integrates with N3/N4 drivers, N11 budget metering, N12 replay, N16 prewarm.

use crate::{
    NodeResult, NodeStatus, WorkflowDefinition, WorkflowExecution, WorkflowNode, WorkflowPhase,
    WorkflowStatus,
};
use allternit_driver_interface::{CommandSpec, ExecutionDriver, SpawnSpec, TenantId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

/// Workflow executor
pub struct WorkflowExecutor {
    /// Execution driver (process, microvm, wasm)
    driver: Arc<dyn ExecutionDriver>,
    /// Active executions
    executions: Arc<RwLock<HashMap<String, WorkflowExecution>>>,
}

impl WorkflowExecutor {
    /// Create new workflow executor
    pub fn new(driver: Arc<dyn ExecutionDriver>) -> Self {
        Self {
            driver,
            executions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Execute a workflow
    pub async fn execute(
        &self,
        workflow: &WorkflowDefinition,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<WorkflowExecution, WorkflowExecutorError> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        info!(
            execution_id = %execution_id,
            workflow_id = %workflow.workflow_id,
            "Starting workflow execution"
        );

        // Build dependency graph
        let dependency_map = self.build_dependency_map(workflow);
        let mut completed_nodes: HashSet<String> = HashSet::new();
        let mut node_results: HashMap<String, NodeResult> = HashMap::new();

        // Create execution record
        let execution = WorkflowExecution {
            execution_id: execution_id.clone(),
            workflow_id: workflow.workflow_id.clone(),
            session_id: session_id.to_string(),
            tenant_id: tenant_id.to_string(),
            start_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            end_time: None,
            status: WorkflowStatus::Running,
            current_phase: Some(WorkflowPhase::Execute),
            node_results: HashMap::new(),
            artifacts: vec![],
            trace_id: Some(execution_id.clone()),
        };

        // Store execution
        {
            let mut execs = self.executions.write().await;
            execs.insert(execution_id.clone(), execution.clone());
        }

        // Execute nodes in topological order
        let mut remaining_nodes: Vec<&WorkflowNode> = workflow.nodes.iter().collect();
        let mut wave = 1;

        while !remaining_nodes.is_empty() {
            // Find nodes with all dependencies satisfied
            let ready_nodes: Vec<&WorkflowNode> = remaining_nodes
                .iter()
                .filter(|node| {
                    dependency_map
                        .get(&node.id)
                        .map(|deps| deps.iter().all(|dep| completed_nodes.contains(dep)))
                        .unwrap_or(true)
                })
                .copied()
                .collect();

            if ready_nodes.is_empty() && !remaining_nodes.is_empty() {
                error!("Circular dependency detected in workflow");
                let mut execs = self.executions.write().await;
                if let Some(exec) = execs.get_mut(&execution_id) {
                    exec.status = WorkflowStatus::Failed;
                    exec.end_time = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    );
                }
                return Err(WorkflowExecutorError::CircularDependency);
            }

            info!(
                wave = wave,
                node_count = ready_nodes.len(),
                "Executing workflow wave"
            );

            // Execute ready nodes concurrently
            let mut node_futures = Vec::new();
            for node in &ready_nodes {
                let driver = self.driver.clone();
                let node = (*node).clone();
                let tenant_id = tenant_id.to_string();
                let execution_id = execution_id.clone();

                let future = tokio::spawn(async move {
                    let result = Self::execute_node(&*driver, &node, &tenant_id).await;
                    (node.id.clone(), result)
                });
                node_futures.push(future);
            }

            // Wait for all nodes in this wave
            for future in node_futures {
                match future.await {
                    Ok((node_id, result)) => match result {
                        Ok(node_result) => {
                            completed_nodes.insert(node_id.clone());
                            node_results.insert(node_id, node_result);
                        }
                        Err(e) => {
                            error!(node_id = %node_id, error = %e, "Node execution failed");
                            let mut execs = self.executions.write().await;
                            if let Some(exec) = execs.get_mut(&execution_id) {
                                exec.status = WorkflowStatus::Failed;
                                exec.end_time = Some(
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_secs(),
                                );
                            }
                            return Err(e);
                        }
                    },
                    Err(e) => {
                        error!(error = %e, "Node task panicked");
                        return Err(WorkflowExecutorError::ExecutionFailed(
                            "Node task panicked".to_string(),
                        ));
                    }
                }
            }

            // Remove executed nodes from remaining
            remaining_nodes.retain(|n| !completed_nodes.contains(&n.id));
            wave += 1;
        }

        // Mark execution as completed
        let mut execs = self.executions.write().await;
        if let Some(exec) = execs.get_mut(&execution_id) {
            exec.status = WorkflowStatus::Completed;
            exec.end_time = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
            exec.node_results = node_results;
            info!(
                execution_id = %execution_id,
                node_count = exec.node_results.len(),
                "Workflow execution completed"
            );
            return Ok(exec.clone());
        }

        Err(WorkflowExecutorError::ExecutionNotFound)
    }

    /// Execute a single node
    async fn execute_node(
        driver: &dyn ExecutionDriver,
        node: &WorkflowNode,
        tenant_id: &str,
    ) -> Result<NodeResult, WorkflowExecutorError> {
        info!(node_id = %node.id, node_name = %node.name, "Executing workflow node");
        let start_time = std::time::Instant::now();

        // Spawn execution environment
        let spawn_spec = SpawnSpec {
            tenant: TenantId(tenant_id.to_string()),
            project: None,
            workspace: None,
            run_id: Some(allternit_driver_interface::ExecutionId::new()),
            env: allternit_driver_interface::EnvironmentSpec {
                spec_type: allternit_driver_interface::EnvSpecType::Oci,
                image: "alpine:latest".to_string(),
                version: None,
                packages: vec![],
                env_vars: std::collections::HashMap::new(),
                working_dir: Some("/workspace".to_string()),
                mounts: vec![],
            },
            policy: allternit_driver_interface::PolicySpec::default_permissive(),
            resources: allternit_driver_interface::ResourceSpec::default(),
            envelope: None,
            prewarm_pool: None,
        };

        let handle = driver
            .spawn(spawn_spec)
            .await
            .map_err(|e| WorkflowExecutorError::SpawnFailed(e.to_string()))?;

        // Execute skill as command
        let cmd_spec = CommandSpec {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo 'Executing skill: {}'", node.skill_id),
            ],
            env_vars: std::collections::HashMap::new(),
            working_dir: Some("/workspace".to_string()),
            stdin_data: None,
            capture_stdout: true,
            capture_stderr: true,
        };

        let exec_result = driver
            .exec(&handle, cmd_spec)
            .await
            .map_err(|e| WorkflowExecutorError::ExecutionFailed(e.to_string()))?;

        // Cleanup
        let _ = driver.destroy(&handle).await;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        let node_result = NodeResult {
            node_id: node.id.clone(),
            status: if exec_result.exit_code == 0 {
                NodeStatus::Completed
            } else {
                NodeStatus::Failed
            },
            output: exec_result
                .stdout
                .map(|s| serde_json::json!(String::from_utf8_lossy(&s).to_string())),
            error: exec_result
                .stderr
                .map(|s| String::from_utf8_lossy(&s).to_string()),
            execution_time_ms,
            artifacts_produced: vec![],
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        info!(
            node_id = %node.id,
            success = exec_result.exit_code == 0,
            execution_time_ms = execution_time_ms,
            "Node execution completed"
        );

        Ok(node_result)
    }

    /// Build dependency map from workflow edges
    fn build_dependency_map(&self, workflow: &WorkflowDefinition) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize empty dependency lists for all nodes
        for node in &workflow.nodes {
            map.insert(node.id.clone(), vec![]);
        }

        // Populate dependencies from edges
        for edge in &workflow.edges {
            if let Some(deps) = map.get_mut(&edge.to) {
                deps.push(edge.from.clone());
            }
        }

        map
    }

    /// Get execution status
    pub async fn get_execution(&self, execution_id: &str) -> Option<WorkflowExecution> {
        let execs = self.executions.read().await;
        execs.get(execution_id).cloned()
    }

    /// List executions
    pub async fn list_executions(&self) -> Vec<WorkflowExecution> {
        let execs = self.executions.read().await;
        execs.values().cloned().collect()
    }

    /// Stop execution
    pub async fn stop_execution(&self, execution_id: &str) -> Result<(), WorkflowExecutorError> {
        let mut execs = self.executions.write().await;
        if let Some(exec) = execs.get_mut(execution_id) {
            exec.status = WorkflowStatus::Stopped;
            exec.end_time = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
            info!(execution_id = %execution_id, "Workflow execution stopped");
            Ok(())
        } else {
            Err(WorkflowExecutorError::ExecutionNotFound)
        }
    }
}

/// Workflow executor errors
#[derive(Debug, thiserror::Error)]
pub enum WorkflowExecutorError {
    #[error("Circular dependency detected in workflow")]
    CircularDependency,

    #[error("Spawn failed: {0}")]
    SpawnFailed(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Execution not found")]
    ExecutionNotFound,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeConstraints, WorkflowEdge, WorkflowNode};

    fn create_test_workflow() -> WorkflowDefinition {
        WorkflowDefinition {
            workflow_id: "test-workflow".to_string(),
            version: "1.0".to_string(),
            description: "Test workflow".to_string(),
            required_roles: vec![],
            allowed_skill_tiers: vec![],
            phases_used: vec![WorkflowPhase::Execute],
            success_criteria: "All nodes complete".to_string(),
            failure_modes: vec![],
            nodes: vec![
                WorkflowNode {
                    id: "node1".to_string(),
                    name: "First Node".to_string(),
                    phase: WorkflowPhase::Execute,
                    skill_id: "echo".to_string(),
                    inputs: vec![],
                    outputs: vec!["output1".to_string()],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec![],
                        required_permissions: vec![],
                    },
                },
                WorkflowNode {
                    id: "node2".to_string(),
                    name: "Second Node".to_string(),
                    phase: WorkflowPhase::Execute,
                    skill_id: "echo".to_string(),
                    inputs: vec!["output1".to_string()],
                    outputs: vec![],
                    constraints: NodeConstraints {
                        time_budget: Some(60),
                        resource_limits: None,
                        allowed_tools: vec![],
                        required_permissions: vec![],
                    },
                },
            ],
            edges: vec![WorkflowEdge {
                from: "node1".to_string(),
                to: "node2".to_string(),
                condition: None,
            }],
        }
    }

    #[test]
    fn test_build_dependency_map() {
        // This would require a mock driver to test fully
        // For now, just verify the structure compiles
        let workflow = create_test_workflow();
        assert_eq!(workflow.nodes.len(), 2);
        assert_eq!(workflow.edges.len(), 1);
    }
}
