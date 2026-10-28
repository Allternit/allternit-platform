use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentTemplateYaml {
    pub version: String,
    pub agent_templates: Vec<AgentTemplateDefinitionYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentTemplateDefinitionYaml {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub tenant_id: String,
    pub persona_ref: Option<String>,
    pub persona_overlay: Option<PersonaOverlayYaml>,
    pub traits: Vec<String>,
    pub roles: Vec<String>,
    pub default_workflow_refs: Vec<String>,
    pub default_task_template_refs: Vec<String>,
    pub skill_policy: Option<SkillPolicyYaml>,
    pub provider_preferences: Option<ProviderPreferencesYaml>,
    pub signing_metadata: Option<SigningMetadataYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PersonaOverlayYaml {
    pub role: String,
    pub goal: String,
    pub backstory: String,
    pub constraints: Vec<String>,
    pub provider_preferences: Option<ProviderPreferencesYaml>,
    pub signature: Option<SignatureYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillPolicyYaml {
    pub allowed_skill_tiers: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProviderPreferencesYaml {
    pub priority_order: Vec<String>,
    pub fallback_timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SigningMetadataYaml {
    pub manifest_sig: String,
    pub bundle_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SignatureYaml {
    pub manifest_sig: String,
    pub bundle_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoicePersonaYaml {
    pub version: String,
    pub voice_personas: Vec<VoicePersonaDefinitionYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoicePersonaDefinitionYaml {
    pub id: String,
    pub persona_ref: String,
    pub voice_profile: VoiceProfileYaml,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoiceProfileYaml {
    pub voice_id: String,
    pub style_labels: Vec<String>,
    pub speaking_rate: Option<f64>,
    pub pitch: Option<f64>,
    pub temperature_voice: Option<f64>,
    pub barge_in_enabled: Option<bool>,
    pub turn_detection: Option<TurnDetectionYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TurnDetectionYaml {
    pub enabled: bool,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraitsYaml {
    pub version: String,
    pub traits: Vec<TraitDefinitionYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraitDefinitionYaml {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub persona_overlay: PersonaOverlayComponentYaml,
    pub tool_constraints: Option<ToolConstraintsYaml>,
    pub workflow_preferences: Option<WorkflowPreferencesYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PersonaOverlayComponentYaml {
    pub context_overlay: Option<ContextOverlayYaml>,
    pub behavior_modifiers: Option<BehaviorModifiersYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextOverlayYaml {
    pub expertise: Option<String>,
    pub methodology: Option<String>,
    pub limitations: Option<String>,
    pub tone_guidelines: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BehaviorModifiersYaml {
    pub risk_tolerance: Option<f64>,
    pub verbosity_level: Option<f64>,
    pub skepticism_level: Option<f64>,
    pub compliance_strictness: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolConstraintsYaml {
    pub preferred_tools: Option<Vec<String>>,
    pub avoided_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowPreferencesYaml {
    pub preferred_phases: Option<Vec<String>>,
    pub time_allocation_bias: Option<HashMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RolesYaml {
    pub version: String,
    pub roles: Vec<RoleDefinitionYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RoleDefinitionYaml {
    pub id: String,
    pub name: String,
    pub description: String,
    pub responsibilities: Vec<String>,
    pub capabilities: CapabilitiesYaml,
    pub permissions: PermissionsYaml,
    pub verification_requirements: VerificationRequirementsYaml,
    pub memory_policy: MemoryPolicyYaml,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CapabilitiesYaml {
    pub allowed_skill_tiers: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_phases: Vec<String>,
    pub resource_limits: Option<ResourceLimitsYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceLimitsYaml {
    pub max_cpu_millicores: Option<u32>,
    pub max_memory_mb: Option<u32>,
    pub max_execution_time_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionsYaml {
    pub required_permissions: Vec<String>,
    pub forbidden_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VerificationRequirementsYaml {
    pub required_verify_artifacts: Vec<String>,
    pub confidence_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryPolicyYaml {
    pub allowed_memory_tiers: Vec<String>,
    pub retention_policy: RetentionPolicyYaml,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetentionPolicyYaml {
    pub max_retention_days: u32,
    pub automatic_consolidation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectTemplatesYaml {
    pub version: String,
    pub project_templates: Vec<ProjectTemplateDefinitionYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectTemplateDefinitionYaml {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub tenant_id: String,
    pub default_agents: Vec<AgentInstanceYaml>,
    pub workflow_bundle: WorkflowBundleYaml,
    pub task_template_bundle: TaskTemplateBundleYaml,
    pub governance_defaults: Option<GovernanceDefaultsYaml>,
    pub ui_preferences: Option<UiPreferencesYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentInstanceYaml {
    pub template_ref: String,
    pub role_assignment: String,
    pub instance_count: u32,
    pub allocation_strategy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowBundleYaml {
    pub default_workflow_ref: String,
    pub alternative_workflows: Vec<String>,
    pub workflow_routing_rules: Vec<WorkflowRoutingRuleYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRoutingRuleYaml {
    pub condition: String,
    pub workflow_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskTemplateBundleYaml {
    pub default_task_templates: Vec<String>,
    pub custom_task_templates: Vec<CustomTaskTemplateYaml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomTaskTemplateYaml {
    pub id: String,
    pub name: String,
    pub definition: TaskTemplateDefinitionYaml,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskTemplateDefinitionYaml {
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GovernanceDefaultsYaml {
    pub default_budgets: Option<BudgetsYaml>,
    pub policy_overrides: Option<Vec<PolicyOverrideYaml>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BudgetsYaml {
    pub max_execution_time_minutes: Option<u64>,
    pub max_tool_calls_per_minute: Option<u32>,
    pub max_tokens_per_minute: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyOverrideYaml {
    pub policy_id: String,
    pub override_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UiPreferencesYaml {
    pub theme: Option<String>,
    pub layout: Option<String>,
    pub default_view: Option<String>,
}

pub struct AgentTemplateCompiler;

impl AgentTemplateCompiler {
    /// Compile an agent template YAML to kernel objects
    pub fn compile_agent_template(&self, yaml_content: &str) -> Result<Vec<AgentTemplateDefinitionYaml>, CompilerError> {
        let agent_template_yaml: AgentTemplateYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| CompilerError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each template
        for template in &agent_template_yaml.agent_templates {
            self.validate_agent_template(template)?;
        }

        Ok(agent_template_yaml.agent_templates)
    }

    /// Compile a voice persona YAML to kernel objects
    pub fn compile_voice_persona(&self, yaml_content: &str) -> Result<Vec<VoicePersonaDefinitionYaml>, CompilerError> {
        let voice_persona_yaml: VoicePersonaYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| CompilerError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each voice persona
        for persona in &voice_persona_yaml.voice_personas {
            self.validate_voice_persona(persona)?;
        }

        Ok(voice_persona_yaml.voice_personas)
    }

    /// Compile traits YAML to kernel objects
    pub fn compile_traits(&self, yaml_content: &str) -> Result<Vec<TraitDefinitionYaml>, CompilerError> {
        let traits_yaml: TraitsYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| CompilerError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each trait
        for trait_def in &traits_yaml.traits {
            self.validate_trait(trait_def)?;
        }

        Ok(traits_yaml.traits)
    }

    /// Compile roles YAML to kernel objects
    pub fn compile_roles(&self, yaml_content: &str) -> Result<Vec<RoleDefinitionYaml>, CompilerError> {
        let roles_yaml: RolesYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| CompilerError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each role
        for role in &roles_yaml.roles {
            self.validate_role(role)?;
        }

        Ok(roles_yaml.roles)
    }

    /// Compile project templates YAML to kernel objects
    pub fn compile_project_templates(&self, yaml_content: &str) -> Result<Vec<ProjectTemplateDefinitionYaml>, CompilerError> {
        let project_templates_yaml: ProjectTemplatesYaml = serde_yaml::from_str(yaml_content)
            .map_err(|e| CompilerError::ParseError(format!("YAML parse error: {}", e)))?;

        // Validate each project template
        for template in &project_templates_yaml.project_templates {
            self.validate_project_template(template)?;
        }

        Ok(project_templates_yaml.project_templates)
    }

    fn validate_agent_template(&self, template: &AgentTemplateDefinitionYaml) -> Result<(), CompilerError> {
        if template.id.is_empty() {
            return Err(CompilerError::ValidationError("Agent template ID cannot be empty".to_string()));
        }

        if template.name.is_empty() {
            return Err(CompilerError::ValidationError("Agent template name cannot be empty".to_string()));
        }

        if template.description.is_empty() {
            return Err(CompilerError::ValidationError("Agent template description cannot be empty".to_string()));
        }

        if template.tenant_id.is_empty() {
            return Err(CompilerError::ValidationError("Agent template tenant_id cannot be empty".to_string()));
        }

        // Validate skill policy if present
        if let Some(policy) = &template.skill_policy {
            for tier in &policy.allowed_skill_tiers {
                match tier.as_str() {
                    "T0" | "T1" | "T2" | "T3" | "T4" => (),
                    _ => return Err(CompilerError::ValidationError(format!("Invalid skill tier: {}", tier))),
                }
            }
        }

        Ok(())
    }

    fn validate_voice_persona(&self, persona: &VoicePersonaDefinitionYaml) -> Result<(), CompilerError> {
        if persona.id.is_empty() {
            return Err(CompilerError::ValidationError("Voice persona ID cannot be empty".to_string()));
        }

        if persona.persona_ref.is_empty() {
            return Err(CompilerError::ValidationError("Voice persona persona_ref cannot be empty".to_string()));
        }

        Ok(())
    }

    fn validate_trait(&self, trait_def: &TraitDefinitionYaml) -> Result<(), CompilerError> {
        if trait_def.id.is_empty() {
            return Err(CompilerError::ValidationError("Trait ID cannot be empty".to_string()));
        }

        if trait_def.name.is_empty() {
            return Err(CompilerError::ValidationError("Trait name cannot be empty".to_string()));
        }

        if trait_def.category.is_empty() {
            return Err(CompilerError::ValidationError("Trait category cannot be empty".to_string()));
        }

        if trait_def.description.is_empty() {
            return Err(CompilerError::ValidationError("Trait description cannot be empty".to_string()));
        }

        Ok(())
    }

    fn validate_role(&self, role: &RoleDefinitionYaml) -> Result<(), CompilerError> {
        if role.id.is_empty() {
            return Err(CompilerError::ValidationError("Role ID cannot be empty".to_string()));
        }

        if role.name.is_empty() {
            return Err(CompilerError::ValidationError("Role name cannot be empty".to_string()));
        }

        if role.description.is_empty() {
            return Err(CompilerError::ValidationError("Role description cannot be empty".to_string()));
        }

        // Validate skill tiers
        for tier in &role.capabilities.allowed_skill_tiers {
            match tier.as_str() {
                "T0" | "T1" | "T2" | "T3" | "T4" => (),
                _ => return Err(CompilerError::ValidationError(format!("Invalid skill tier: {}", tier))),
            }
        }

        // Validate phases
        for phase in &role.capabilities.allowed_phases {
            match phase.as_str() {
                "Observe" | "Think" | "Plan" | "Build" | "Execute" | "Verify" | "Learn" => (),
                _ => return Err(CompilerError::ValidationError(format!("Invalid workflow phase: {}", phase))),
            }
        }

        Ok(())
    }

    fn validate_project_template(&self, template: &ProjectTemplateDefinitionYaml) -> Result<(), CompilerError> {
        if template.id.is_empty() {
            return Err(CompilerError::ValidationError("Project template ID cannot be empty".to_string()));
        }

        if template.name.is_empty() {
            return Err(CompilerError::ValidationError("Project template name cannot be empty".to_string()));
        }

        if template.description.is_empty() {
            return Err(CompilerError::ValidationError("Project template description cannot be empty".to_string()));
        }

        if template.tenant_id.is_empty() {
            return Err(CompilerError::ValidationError("Project template tenant_id cannot be empty".to_string()));
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompilerError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Reference error: {0}")]
    ReferenceError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_valid_agent_template() {
        let yaml_content = r#"
version: "1.0"
agent_templates:
  - id: "test-agent-template"
    name: "Test Agent Template"
    description: "A test agent template"
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

        let compiler = AgentTemplateCompiler;
        let result = compiler.compile_agent_template(yaml_content);

        assert!(result.is_ok());
        let templates = result.unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].id, "test-agent-template");
    }

    #[test]
    fn test_compile_valid_voice_persona() {
        let yaml_content = r#"
version: "1.0"
voice_personas:
  - id: "test-voice-persona"
    persona_ref: "test-persona"
    voice_profile:
      voice_id: "test-voice"
      style_labels: ["calm", "professional"]
      speaking_rate: 0.9
      pitch: 0.1
      temperature_voice: 0.2
      barge_in_enabled: false
      turn_detection:
        enabled: true
        timeout_ms: 2000
"#;

        let compiler = AgentTemplateCompiler;
        let result = compiler.compile_voice_persona(yaml_content);

        assert!(result.is_ok());
        let personas = result.unwrap();
        assert_eq!(personas.len(), 1);
        assert_eq!(personas[0].id, "test-voice-persona");
    }

    #[test]
    fn test_compile_valid_trait() {
        let yaml_content = r#"
version: "1.0"
traits:
  - id: "test-trait"
    name: "Test Trait"
    category: "analytical"
    description: "A test trait"
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

        let compiler = AgentTemplateCompiler;
        let result = compiler.compile_traits(yaml_content);

        assert!(result.is_ok());
        let traits = result.unwrap();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].id, "test-trait");
    }

    #[test]
    fn test_compile_valid_role() {
        let yaml_content = r#"
version: "1.0"
roles:
  - id: "test-role"
    name: "Test Role"
    description: "A test role"
    responsibilities: ["resp1", "resp2"]
    capabilities:
      allowed_skill_tiers: ["T0", "T1"]
      allowed_tools: ["tool1", "tool2"]
      allowed_phases: ["Observe", "Think", "Plan"]
      resource_limits:
        max_cpu_millicores: 500
        max_memory_mb: 1024
        max_execution_time_seconds: 3600
    permissions:
      required_permissions: ["perm1", "perm2"]
      forbidden_actions: ["action1", "action2"]
    verification_requirements:
      required_verify_artifacts: ["artifact1", "artifact2"]
      confidence_threshold: 0.85
    memory_policy:
      allowed_memory_tiers: ["Working", "Episodic", "Semantic"]
      retention_policy:
        max_retention_days: 30
        automatic_consolidation: true
"#;

        let compiler = AgentTemplateCompiler;
        let result = compiler.compile_roles(yaml_content);

        assert!(result.is_ok());
        let roles = result.unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].id, "test-role");
    }

    #[test]
    fn test_compile_valid_project_template() {
        let yaml_content = r#"
version: "1.0"
project_templates:
  - id: "test-project-template"
    name: "Test Project Template"
    description: "A test project template"
    version: "1.0.0"
    tenant_id: "tenant-123"
    default_agents:
      - template_ref: "test-agent-template"
        role_assignment: "test-role"
        instance_count: 1
        allocation_strategy: "dedicated"
    workflow_bundle:
      default_workflow_ref: "test-workflow"
      alternative_workflows: ["alt1", "alt2"]
      workflow_routing_rules:
        - condition: "condition1"
          workflow_ref: "workflow1"
    task_template_bundle:
      default_task_templates: ["task1", "task2"]
      custom_task_templates:
        - id: "custom-task"
          name: "Custom Task"
          definition:
            input_schema:
              type: "object"
              properties:
                input:
                  type: "string"
              required: ["input"]
            output_schema:
              type: "object"
              properties:
                output:
                  type: "string"
              required: ["output"]
    governance_defaults:
      default_budgets:
        max_execution_time_minutes: 1440
        max_tool_calls_per_minute: 60
        max_tokens_per_minute: 10000
    ui_preferences:
      theme: "test-theme"
      layout: "dashboard"
      default_view: "progress_tracker"
"#;

        let compiler = AgentTemplateCompiler;
        let result = compiler.compile_project_templates(yaml_content);

        assert!(result.is_ok());
        let templates = result.unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].id, "test-project-template");
    }

    #[test]
    fn test_compile_invalid_agent_template() {
        let yaml_content = r#"
version: "1.0"
agent_templates:
  - id: ""
    name: ""
    description: ""
    version: "1.0.0"
    tenant_id: ""
"#;

        let compiler = AgentTemplateCompiler;
        let result = compiler.compile_agent_template(yaml_content);

        assert!(result.is_err());
    }
}