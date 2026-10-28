use super::compiler::YamlCompiler;
use super::validator::YamlValidator;
use crate::WorkflowDefinition;
use allternit_policy::PolicyEngine;
use std::path::Path;
use std::sync::Arc;

pub struct YamlLoader {
    compiler: YamlCompiler,
    validator: YamlValidator,
    workflow_registry:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, WorkflowDefinition>>>,
}

impl YamlLoader {
    pub fn new(policy_engine: Arc<PolicyEngine>, tenant_id: String) -> Self {
        let compiler = YamlCompiler;
        let validator = YamlValidator::new(policy_engine.clone(), tenant_id.clone());
        let workflow_registry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        YamlLoader {
            compiler,
            validator,
            workflow_registry,
        }
    }

    /// Load and compile a workflow from a YAML file
    pub fn load_workflow_from_file<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<WorkflowDefinition, LoaderError> {
        // Read the file content
        let yaml_content =
            std::fs::read_to_string(file_path).map_err(|e| LoaderError::IoError(e.to_string()))?;

        // Validate the YAML content
        self.validator.validate_workflow(&yaml_content)?;

        // Compile the YAML to kernel objects
        let workflow_def = self.compiler.compile_workflow(&yaml_content)?;

        // Register the workflow in the registry
        {
            let mut registry = self
                .workflow_registry
                .lock()
                .map_err(|e| LoaderError::MutexPoisoned(e.to_string()))?;
            registry.insert(workflow_def.workflow_id.clone(), workflow_def.clone());
        }

        Ok(workflow_def)
    }

    /// Load and compile multiple workflows from a directory
    pub fn load_workflows_from_directory<P: AsRef<Path>>(
        &self,
        dir_path: P,
    ) -> Result<Vec<WorkflowDefinition>, LoaderError> {
        let mut workflows = Vec::new();

        let entries =
            std::fs::read_dir(dir_path).map_err(|e| LoaderError::IoError(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| LoaderError::IoError(e.to_string()))?;
            let path = entry.path();

            if path
                .extension()
                .is_some_and(|ext| ext == "yaml" || ext == "yml")
            {
                match self.load_workflow_from_file(&path) {
                    Ok(workflow) => workflows.push(workflow),
                    Err(e) => return Err(e), // In a more robust implementation, we might collect errors instead of failing fast
                }
            }
        }

        Ok(workflows)
    }

    /// Get a workflow from the registry by ID
    pub fn get_workflow(&self, workflow_id: &str) -> Option<WorkflowDefinition> {
        let registry = self
            .workflow_registry
            .lock()
            .map_err(|e| LoaderError::MutexPoisoned(e.to_string()))
            .ok()?;
        registry.get(workflow_id).cloned()
    }

    /// List all loaded workflows
    pub fn list_workflows(&self) -> Vec<WorkflowDefinition> {
        self.workflow_registry
            .lock()
            .map_err(|e| LoaderError::MutexPoisoned(e.to_string()))
            .unwrap()
            .values()
            .cloned()
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Compiler error: {0}")]
    CompilerError(#[from] super::compiler::CompilerError),
    #[error("Validation error: {0}")]
    ValidationError(#[from] super::validator::ValidationError),
    #[error("Mutex poisoned: {0}")]
    MutexPoisoned(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_load_valid_workflow() {
        // Create a temporary YAML file with valid workflow content
        let mut temp_file = NamedTempFile::new().unwrap();
        let yaml_content = r#"
version: "1.0"
workflow:
  id: "test-load-workflow"
  name: "Test Load Workflow"
  description: "A test workflow for loading"
  version: "1.0.0"
  tenant_id: "tenant-123"
  required_tiers:
    - "T0"
    - "T1"
  success_criteria: "All tasks completed successfully"
  failure_modes:
    - "Any task fails"
  phases:
    - "Observe"
    - "Think"
    - "Plan"
  tasks:
    - id: "task-1"
      name: "First Task"
      phase: "Observe"
      persona: "researcher"
      description: "First task in workflow"
      instructions: "Perform initial observation"
      inputs: []
      outputs: ["observation_result"]
      tools: ["search_tool"]
  dependencies: []
"#;
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        // Set up dependencies
        let temp_db_path = format!("/tmp/test_loader_db_{}.db", Uuid::new_v4());
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", temp_db_path))
            .await
            .unwrap();
        let temp_history_path = format!("/tmp/test_loader_history_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(std::sync::Mutex::new(
            allternit_history::HistoryLedger::new(&temp_history_path).unwrap(),
        ));
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Register a default identity for testing
        let identity = allternit_policy::Identity {
            id: "workflow_loader".to_string(),
            identity_type: allternit_policy::IdentityType::AgentIdentity,
            name: "workflow_loader".to_string(),
            tenant_id: "tenant-123".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["user".to_string()],
            permissions: vec!["perm_t0_read".to_string(), "perm_t1_read".to_string()],
        };
        policy_engine.register_identity(identity).await.unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        policy_engine.create_default_rules().await.unwrap();

        let loader = YamlLoader::new(policy_engine, "tenant-123".to_string());

        // Load the workflow
        let result = loader.load_workflow_from_file(temp_file.path());
        assert!(result.is_ok());

        let workflow_def = result.unwrap();
        assert_eq!(workflow_def.workflow_id, "test-load-workflow");
        assert_eq!(workflow_def.nodes.len(), 1);

        // Verify the workflow was registered
        let retrieved_workflow = loader.get_workflow("test-load-workflow");
        assert!(retrieved_workflow.is_some());
        assert_eq!(
            retrieved_workflow.unwrap().workflow_id,
            "test-load-workflow"
        );
    }

    #[tokio::test]
    async fn test_load_invalid_workflow() {
        // Create a temporary YAML file with invalid workflow content
        let mut temp_file = NamedTempFile::new().unwrap();
        let yaml_content = r#"
version: "1.0"
workflow:
  id: "test-invalid-workflow"
  name: "Test Invalid Workflow"
  description: "A test workflow with invalid content"
  version: "1.0.0"
  tenant_id: "different-tenant"  # This should fail tenant validation
  required_tiers:
    - "T0"
  success_criteria: "All tasks completed successfully"
  failure_modes:
    - "Any task fails"
  phases:
    - "InvalidPhase"  # This phase doesn't exist
  tasks:
    - id: "task-1"
      name: "First Task"
      phase: "InvalidPhase"  # This phase doesn't exist
      persona: "researcher"
      description: "First task in workflow"
      instructions: "Perform initial observation"
      inputs: []
      outputs: ["observation_result"]
      tools: ["search_tool"]
  dependencies: []
"#;
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        // Set up dependencies
        let temp_db_path = format!("/tmp/test_loader_invalid_db_{}.db", Uuid::new_v4());
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", temp_db_path))
            .await
            .unwrap();
        let temp_history_path =
            format!("/tmp/test_loader_invalid_history_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(std::sync::Mutex::new(
            allternit_history::HistoryLedger::new(&temp_history_path).unwrap(),
        ));
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let loader = YamlLoader::new(policy_engine, "tenant-123".to_string());

        // Load the workflow - should fail
        let result = loader.load_workflow_from_file(temp_file.path());
        assert!(result.is_err());
    }
}
