use crate::agent_template_compiler::{
    AgentTemplateCompiler, AgentTemplateDefinitionYaml, ProjectTemplateDefinitionYaml,
    RoleDefinitionYaml, TraitDefinitionYaml, VoicePersonaDefinitionYaml,
};
use crate::agent_template_validator::AgentTemplateValidator;
use allternit_policy::PolicyEngine;
use std::sync::Arc;
use std::path::Path;

pub struct AgentTemplateLoader {
    compiler: AgentTemplateCompiler,
    validator: AgentTemplateValidator,
}

impl AgentTemplateLoader {
    pub fn new(policy_engine: Arc<PolicyEngine>, tenant_id: String) -> Self {
        let compiler = AgentTemplateCompiler;
        let validator = AgentTemplateValidator::new(policy_engine, tenant_id.clone());
        
        AgentTemplateLoader {
            compiler,
            validator,
        }
    }

    /// Load and compile an agent template from a YAML file
    pub async fn load_agent_template_from_file<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<AgentTemplateDefinitionYaml>, LoaderError> {
        // Read the file content
        let yaml_content = std::fs::read_to_string(file_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        // Validate the YAML content
        self.validator.validate_agent_template(&yaml_content)?;

        // Compile the YAML to kernel objects
        let agent_templates = self.compiler.compile_agent_template(&yaml_content)?;

        Ok(agent_templates)
    }

    /// Load and compile voice personas from a YAML file
    pub async fn load_voice_personas_from_file<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<VoicePersonaDefinitionYaml>, LoaderError> {
        // Read the file content
        let yaml_content = std::fs::read_to_string(file_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        // Validate the YAML content
        self.validator.validate_voice_persona(&yaml_content)?;

        // Compile the YAML to kernel objects
        let voice_personas = self.compiler.compile_voice_persona(&yaml_content)?;

        Ok(voice_personas)
    }

    /// Load and compile traits from a YAML file
    pub async fn load_traits_from_file<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<TraitDefinitionYaml>, LoaderError> {
        // Read the file content
        let yaml_content = std::fs::read_to_string(file_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        // Validate the YAML content
        self.validator.validate_traits(&yaml_content)?;

        // Compile the YAML to kernel objects
        let traits = self.compiler.compile_traits(&yaml_content)?;

        Ok(traits)
    }

    /// Load and compile roles from a YAML file
    pub async fn load_roles_from_file<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<RoleDefinitionYaml>, LoaderError> {
        // Read the file content
        let yaml_content = std::fs::read_to_string(file_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        // Validate the YAML content
        self.validator.validate_roles(&yaml_content)?;

        // Compile the YAML to kernel objects
        let roles = self.compiler.compile_roles(&yaml_content)?;

        Ok(roles)
    }

    /// Load and compile project templates from a YAML file
    pub async fn load_project_templates_from_file<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<ProjectTemplateDefinitionYaml>, LoaderError> {
        // Read the file content
        let yaml_content = std::fs::read_to_string(file_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        // Validate the YAML content
        self.validator.validate_project_templates(&yaml_content)?;

        // Compile the YAML to kernel objects
        let project_templates = self.compiler.compile_project_templates(&yaml_content)?;

        Ok(project_templates)
    }

    /// Load and compile multiple agent templates from a directory
    pub async fn load_agent_templates_from_directory<P: AsRef<Path>>(
        &self,
        dir_path: P,
    ) -> Result<Vec<AgentTemplateDefinitionYaml>, LoaderError> {
        let mut all_templates = Vec::new();
        
        let entries = std::fs::read_dir(dir_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| LoaderError::IoError(e.to_string()))?;
            let path = entry.path();
            
            if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                match self.load_agent_template_from_file(&path).await {
                    Ok(templates) => all_templates.extend(templates),
                    Err(e) => return Err(e), // In a more robust implementation, we might collect errors instead of failing fast
                }
            }
        }

        Ok(all_templates)
    }

    /// Load and compile multiple voice personas from a directory
    pub async fn load_voice_personas_from_directory<P: AsRef<Path>>(
        &self,
        dir_path: P,
    ) -> Result<Vec<VoicePersonaDefinitionYaml>, LoaderError> {
        let mut all_personas = Vec::new();
        
        let entries = std::fs::read_dir(dir_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| LoaderError::IoError(e.to_string()))?;
            let path = entry.path();
            
            if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                match self.load_voice_personas_from_file(&path).await {
                    Ok(personas) => all_personas.extend(personas),
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(all_personas)
    }

    /// Load and compile multiple traits from a directory
    pub async fn load_traits_from_directory<P: AsRef<Path>>(
        &self,
        dir_path: P,
    ) -> Result<Vec<TraitDefinitionYaml>, LoaderError> {
        let mut all_traits = Vec::new();
        
        let entries = std::fs::read_dir(dir_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| LoaderError::IoError(e.to_string()))?;
            let path = entry.path();
            
            if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                match self.load_traits_from_file(&path).await {
                    Ok(traits) => all_traits.extend(traits),
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(all_traits)
    }

    /// Load and compile multiple roles from a directory
    pub async fn load_roles_from_directory<P: AsRef<Path>>(
        &self,
        dir_path: P,
    ) -> Result<Vec<RoleDefinitionYaml>, LoaderError> {
        let mut all_roles = Vec::new();
        
        let entries = std::fs::read_dir(dir_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| LoaderError::IoError(e.to_string()))?;
            let path = entry.path();
            
            if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                match self.load_roles_from_file(&path).await {
                    Ok(roles) => all_roles.extend(roles),
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(all_roles)
    }

    /// Load and compile multiple project templates from a directory
    pub async fn load_project_templates_from_directory<P: AsRef<Path>>(
        &self,
        dir_path: P,
    ) -> Result<Vec<ProjectTemplateDefinitionYaml>, LoaderError> {
        let mut all_templates = Vec::new();
        
        let entries = std::fs::read_dir(dir_path)
            .map_err(|e| LoaderError::IoError(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| LoaderError::IoError(e.to_string()))?;
            let path = entry.path();
            
            if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                match self.load_project_templates_from_file(&path).await {
                    Ok(templates) => all_templates.extend(templates),
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(all_templates)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Compiler error: {0}")]
    CompilerError(#[from] crate::agent_template_compiler::CompilerError),
    #[error("Validation error: {0}")]
    ValidationError(#[from] crate::agent_template_validator::ValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use std::io::Write;
    use tokio;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_load_valid_agent_template() {
        // Create a temporary YAML file with valid agent template content
        let mut temp_file = NamedTempFile::new().unwrap();
        let yaml_content = r#"
version: "1.0"
agent_templates:
  - id: "test-load-agent"
    name: "Test Load Agent"
    description: "A test agent for loading"
    version: "1.0.0"
    tenant_id: "tenant-123"
    persona_overlay:
      role: "Test Role"
      goal: "Test Goal"
      backstory: "Test Backstory"
      constraints: ["constraint1"]
    traits: ["trait1"]
    roles: ["role1"]
    default_workflow_refs: ["workflow1"]
    default_task_template_refs: ["task1"]
    skill_policy:
      allowed_skill_tiers: ["T0", "T1"]
      allowed_tools: ["tool1"]
      denied_tools: ["tool2"]
    provider_preferences:
      priority_order: ["provider1", "provider2"]
      fallback_timeout: 30
    signing_metadata:
      manifest_sig: "test-sig"
      bundle_hash: "test-hash"
"#;
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        // Set up dependencies
        let temp_db_path = format!("/tmp/test_loader_db_{}.db", Uuid::new_v4());
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", temp_db_path)).await.unwrap();
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
            id: "agent_template_loader".to_string(),
            identity_type: allternit_policy::IdentityType::AgentIdentity,
            name: "Agent Template Loader".to_string(),
            tenant_id: "tenant-123".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["user".to_string()],
            permissions: vec!["perm_t0_read".to_string(), "perm_t1_read".to_string()],
        };
        policy_engine.register_identity(identity).await.unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        policy_engine.create_default_rules().await.unwrap();

        let loader = AgentTemplateLoader::new(policy_engine, "tenant-123".to_string());

        // Load the agent template
        let result = loader.load_agent_template_from_file(temp_file.path()).await;
        assert!(result.is_ok());

        let templates = result.unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].id, "test-load-agent");
    }

    #[tokio::test]
    async fn test_load_invalid_agent_template() {
        // Create a temporary YAML file with invalid agent template content
        let mut temp_file = NamedTempFile::new().unwrap();
        let yaml_content = r#"
version: "1.0"
agent_templates:
  - id: ""  # This should fail validation
    name: ""
    description: ""
    version: "1.0.0"
    tenant_id: ""
    persona_overlay:
      role: ""
      goal: ""
      backstory: ""
      constraints: []
"#;
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        // Set up dependencies
        let temp_db_path = format!("/tmp/test_loader_invalid_db_{}.db", Uuid::new_v4());
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", temp_db_path)).await.unwrap();
        let temp_history_path = format!("/tmp/test_loader_invalid_history_{}.jsonl", Uuid::new_v4());
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

        let loader = AgentTemplateLoader::new(policy_engine, "tenant-123".to_string());

        // Load the agent template - should fail
        let result = loader.load_agent_template_from_file(temp_file.path()).await;
        assert!(result.is_err());
    }
}
