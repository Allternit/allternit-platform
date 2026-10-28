use crate::agent_template_compiler::{
    AgentTemplateDefinitionYaml, AgentTemplateYaml, ProjectTemplateDefinitionYaml,
    ProjectTemplatesYaml, RoleDefinitionYaml, RolesYaml, TraitDefinitionYaml, TraitsYaml,
    VoicePersonaDefinitionYaml, VoicePersonaYaml,
};
use allternit_policy::PolicyEngine;
use std::sync::Arc;

pub struct AgentTemplateValidator {
    policy_engine: Arc<PolicyEngine>,
    tenant_id: String,
}

impl AgentTemplateValidator {
    pub fn new(policy_engine: Arc<PolicyEngine>, tenant_id: String) -> Self {
        AgentTemplateValidator {
            policy_engine,
            tenant_id,
        }
    }

    /// Validate an agent template YAML against kernel contracts and policies
    pub fn validate_agent_template(&self, yaml_content: &str) -> Result<(), ValidationError> {
        // First, parse the YAML to ensure it's syntactically valid
        let agent_template_yaml: AgentTemplateYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| ValidationError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate tenant isolation
        for template in &agent_template_yaml.agent_templates {
            if template.tenant_id != self.tenant_id {
                return Err(ValidationError::TenantIsolationError(
                    format!("Template tenant_id '{}' does not match validator tenant_id '{}'",
                            template.tenant_id, self.tenant_id)
                ));
            }
        }

        // Validate each template
        for template in &agent_template_yaml.agent_templates {
            self.validate_agent_template_structure(template)?;
        }

        Ok(())
    }

    /// Validate a voice persona YAML against kernel contracts and policies
    pub fn validate_voice_persona(&self, yaml_content: &str) -> Result<(), ValidationError> {
        // First, parse the YAML to ensure it's syntactically valid
        let voice_persona_yaml: VoicePersonaYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| ValidationError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each voice persona
        for persona in &voice_persona_yaml.voice_personas {
            self.validate_voice_persona_structure(persona)?;
        }

        Ok(())
    }

    /// Validate traits YAML against kernel contracts
    pub fn validate_traits(&self, yaml_content: &str) -> Result<(), ValidationError> {
        // First, parse the YAML to ensure it's syntactically valid
        let traits_yaml: TraitsYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| ValidationError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each trait
        for trait_def in &traits_yaml.traits {
            self.validate_trait_structure(trait_def)?;
        }

        Ok(())
    }

    /// Validate roles YAML against kernel contracts and policies
    pub fn validate_roles(&self, yaml_content: &str) -> Result<(), ValidationError> {
        // First, parse the YAML to ensure it's syntactically valid
        let roles_yaml: RolesYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| ValidationError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each role
        for role in &roles_yaml.roles {
            self.validate_role_structure(role)?;
        }

        Ok(())
    }

    /// Validate project templates YAML against kernel contracts and policies
    pub fn validate_project_templates(&self, yaml_content: &str) -> Result<(), ValidationError> {
        // First, parse the YAML to ensure it's syntactically valid
        let project_templates_yaml: ProjectTemplatesYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| ValidationError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate tenant isolation
        for template in &project_templates_yaml.project_templates {
            if template.tenant_id != self.tenant_id {
                return Err(ValidationError::TenantIsolationError(
                    format!("Template tenant_id '{}' does not match validator tenant_id '{}'",
                            template.tenant_id, self.tenant_id)
                ));
            }
        }

        // Validate each project template
        for template in &project_templates_yaml.project_templates {
            self.validate_project_template_structure(template)?;
        }

        Ok(())
    }

    fn validate_agent_template_structure(&self, template: &AgentTemplateDefinitionYaml) -> Result<(), ValidationError> {
        if template.id.is_empty() {
            return Err(ValidationError::ValidationError("Agent template ID cannot be empty".to_string()));
        }

        if template.name.is_empty() {
            return Err(ValidationError::ValidationError("Agent template name cannot be empty".to_string()));
        }

        if template.description.is_empty() {
            return Err(ValidationError::ValidationError("Agent template description cannot be empty".to_string()));
        }

        if template.tenant_id.is_empty() {
            return Err(ValidationError::ValidationError("Agent template tenant_id cannot be empty".to_string()));
        }

        // Validate persona reference if provided
        if let Some(persona_ref) = &template.persona_ref {
            if persona_ref.is_empty() {
                return Err(ValidationError::ValidationError("Persona reference cannot be empty".to_string()));
            }
        }

        // Validate persona overlay if provided
        if let Some(overlay) = &template.persona_overlay {
            if overlay.role.is_empty() {
                return Err(ValidationError::ValidationError("Persona overlay role cannot be empty".to_string()));
            }

            if overlay.goal.is_empty() {
                return Err(ValidationError::ValidationError("Persona overlay goal cannot be empty".to_string()));
            }

            if overlay.backstory.is_empty() {
                return Err(ValidationError::ValidationError("Persona overlay backstory cannot be empty".to_string()));
            }
        }

        // Validate skill policy if present
        if let Some(policy) = &template.skill_policy {
            for tier in &policy.allowed_skill_tiers {
                match tier.as_str() {
                    "T0" | "T1" | "T2" | "T3" | "T4" => (),
                    _ => return Err(ValidationError::ValidationError(format!("Invalid skill tier: {}", tier))),
                }
            }
        }

        Ok(())
    }

    fn validate_voice_persona_structure(&self, persona: &VoicePersonaDefinitionYaml) -> Result<(), ValidationError> {
        if persona.id.is_empty() {
            return Err(ValidationError::ValidationError("Voice persona ID cannot be empty".to_string()));
        }

        if persona.persona_ref.is_empty() {
            return Err(ValidationError::ValidationError("Voice persona persona_ref cannot be empty".to_string()));
        }

        if persona.voice_profile.voice_id.is_empty() {
            return Err(ValidationError::ValidationError("Voice profile voice_id cannot be empty".to_string()));
        }

        Ok(())
    }

    fn validate_trait_structure(&self, trait_def: &TraitDefinitionYaml) -> Result<(), ValidationError> {
        if trait_def.id.is_empty() {
            return Err(ValidationError::ValidationError("Trait ID cannot be empty".to_string()));
        }

        if trait_def.name.is_empty() {
            return Err(ValidationError::ValidationError("Trait name cannot be empty".to_string()));
        }

        if trait_def.category.is_empty() {
            return Err(ValidationError::ValidationError("Trait category cannot be empty".to_string()));
        }

        if trait_def.description.is_empty() {
            return Err(ValidationError::ValidationError("Trait description cannot be empty".to_string()));
        }

        Ok(())
    }

    fn validate_role_structure(&self, role: &RoleDefinitionYaml) -> Result<(), ValidationError> {
        if role.id.is_empty() {
            return Err(ValidationError::ValidationError("Role ID cannot be empty".to_string()));
        }

        if role.name.is_empty() {
            return Err(ValidationError::ValidationError("Role name cannot be empty".to_string()));
        }

        if role.description.is_empty() {
            return Err(ValidationError::ValidationError("Role description cannot be empty".to_string()));
        }

        // Validate skill tiers
        for tier in &role.capabilities.allowed_skill_tiers {
            match tier.as_str() {
                "T0" | "T1" | "T2" | "T3" | "T4" => (),
                _ => return Err(ValidationError::ValidationError(format!("Invalid skill tier: {}", tier))),
            }
        }

        // Validate phases
        for phase in &role.capabilities.allowed_phases {
            match phase.as_str() {
                "Observe" | "Think" | "Plan" | "Build" | "Execute" | "Verify" | "Learn" => (),
                _ => return Err(ValidationError::ValidationError(format!("Invalid workflow phase: {}", phase))),
            }
        }

        Ok(())
    }

    fn validate_project_template_structure(&self, template: &ProjectTemplateDefinitionYaml) -> Result<(), ValidationError> {
        if template.id.is_empty() {
            return Err(ValidationError::ValidationError("Project template ID cannot be empty".to_string()));
        }

        if template.name.is_empty() {
            return Err(ValidationError::ValidationError("Project template name cannot be empty".to_string()));
        }

        if template.description.is_empty() {
            return Err(ValidationError::ValidationError("Project template description cannot be empty".to_string()));
        }

        if template.tenant_id.is_empty() {
            return Err(ValidationError::ValidationError("Project template tenant_id cannot be empty".to_string()));
        }

        // Validate default agents
        for agent in &template.default_agents {
            if agent.template_ref.is_empty() {
                return Err(ValidationError::ValidationError("Agent template_ref cannot be empty".to_string()));
            }

            if agent.role_assignment.is_empty() {
                return Err(ValidationError::ValidationError("Agent role_assignment cannot be empty".to_string()));
            }

            if agent.instance_count == 0 {
                return Err(ValidationError::ValidationError("Agent instance_count must be greater than 0".to_string()));
            }
        }

        // Validate workflow bundle
        if template.workflow_bundle.default_workflow_ref.is_empty() {
            return Err(ValidationError::ValidationError("Workflow bundle default_workflow_ref cannot be empty".to_string()));
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Tenant isolation error: {0}")]
    TenantIsolationError(String),
    #[error("Policy error: {0}")]
    PolicyError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_validate_valid_agent_template() {
        let temp_path = format!("/tmp/test_validator_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(std::sync::Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool,
            )
            .await
            .unwrap(),
        );
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let validator = AgentTemplateValidator::new(policy_engine, "tenant-123".to_string());

        let yaml_content = r#"
version: "1.0"
agent_templates:
  - id: "test-validate-agent"
    name: "Test Validate Agent"
    description: "A test agent for validation"
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

        let result = validator.validate_agent_template(yaml_content);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_invalid_tenant() {
        let temp_path = format!("/tmp/test_validator2_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(std::sync::Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool,
            )
            .await
            .unwrap(),
        );
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let validator = AgentTemplateValidator::new(policy_engine, "tenant-123".to_string());

        let yaml_content = r#"
version: "1.0"
agent_templates:
  - id: "test-invalid-tenant"
    name: "Test Invalid Tenant Agent"
    description: "A test agent with invalid tenant"
    version: "1.0.0"
    tenant_id: "different-tenant"  # This should fail tenant validation
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

        let result = validator.validate_agent_template(yaml_content);
        assert!(result.is_err());
        match result.unwrap_err() {
            ValidationError::TenantIsolationError(_) => (), // Expected
            _ => panic!("Expected TenantIsolationError"),
        }
    }

    #[tokio::test]
    async fn test_validate_valid_traits() {
        let temp_path = format!("/tmp/test_traits_validator_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(std::sync::Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool,
            )
            .await
            .unwrap(),
        );
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let validator = AgentTemplateValidator::new(policy_engine, "tenant-123".to_string());

        let yaml_content = r#"
version: "1.0"
traits:
  - id: "test-trait-validate"
    name: "Test Trait Validate"
    category: "analytical"
    description: "A test trait for validation"
    persona_overlay:
      context_overlay:
        expertise: "Test expertise"
        methodology: "Test methodology"
        limitations: "Test limitations"
        tone_guidelines: ["be clear", "be concise"]
      behavior_modifiers:
        risk_tolerance: 0.5
        verbosity_level: 0.8
        skepticism_level: 0.7
        compliance_strictness: 0.9
"#;

        let result = validator.validate_traits(yaml_content);
        assert!(result.is_ok());
    }
}
