//! Allternit Hooks System
//!
//! Implements the Hooks System for Allternit:
//! - Kernel Hooks (boot injection, tool gates, policy enforcement)
//! - Workspace Hooks (SYSTEM_LAW validation, spec presence, ADR enforcement)
//! - Task Hooks (output schema validation, structured formatting)
//! - Human Layer Hooks (escalation gates, approval triggers)
//! - Habit Promotion Protocol (determinism test, rollback mechanism)
//!
//! Based on HooksSystem specification from brainstorm sessions

use allternit_harness_engineering::{HarnessEngineeringEngine, RiskTier};
use allternit_system_law::{SystemLawEngine, ViolationSeverity, LawViolation};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ============================================================================
// Hook Types and Definitions
// ============================================================================

/// Hook layer classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookLayer {
    /// Kernel layer - boot injection, tool gates, policy enforcement
    Kernel,
    /// Workspace layer - SYSTEM_LAW validation, spec presence
    Workspace,
    /// Task layer - output schema validation, structured formatting
    Task,
    /// Human layer - escalation gates, approval triggers
    Human,
}

/// Hook event type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    /// Boot sequence event
    Boot(BootEvent),
    /// Tool invocation event
    ToolInvoke(ToolInvokeEvent),
    /// File system event
    FileSystem(FileSystemEvent),
    /// Spec validation event
    SpecValidation(SpecValidationEvent),
    /// Task output event
    TaskOutput(TaskOutputEvent),
    /// Escalation event
    Escalation(EscalationEvent),
    /// Approval event
    Approval(ApprovalEvent),
}

/// Boot event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootEvent {
    pub phase: String,
    pub component: String,
    pub success: bool,
    pub timestamp: DateTime<Utc>,
}

/// Tool invocation event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvokeEvent {
    pub tool_id: String,
    pub tool_type: ToolType,
    pub safety_tier: SafetyTier,
    pub preconditions_met: bool,
    pub timestamp: DateTime<Utc>,
}

/// Tool type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolType {
    Read,
    Write,
    Destructive,
    Network,
}

/// Safety tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SafetyTier {
    Safe,
    Caution,
    Dangerous,
    Critical,
}

/// File system event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemEvent {
    pub operation: FileOperation,
    pub path: String,
    pub allowed: bool,
    pub reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// File operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileOperation {
    Read,
    Write,
    Delete,
    Rename,
}

/// Spec validation event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecValidationEvent {
    pub spec_type: String,
    pub present: bool,
    pub valid: bool,
    pub errors: Vec<String>,
    pub timestamp: DateTime<Utc>,
}

/// Task output event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutputEvent {
    pub task_id: String,
    pub output_type: String,
    pub schema_valid: bool,
    pub formatted: bool,
    pub verified: bool,
    pub timestamp: DateTime<Utc>,
}

/// Escalation event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationEvent {
    pub from_level: EscalationLevel,
    pub to_level: EscalationLevel,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Escalation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EscalationLevel {
    Agent,
    SeniorAgent,
    Human,
    Security,
}

/// Approval event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEvent {
    pub approval_type: String,
    pub approved: bool,
    pub approver: String,
    pub reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Hook Definition
// ============================================================================

/// Hook definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub hook_id: String,
    pub name: String,
    pub layer: HookLayer,
    pub event_types: Vec<String>,
    pub handler: String,
    pub priority: u32,
    pub blocking: bool,
    pub enabled: bool,
}

/// Hook execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub hook_id: String,
    pub success: bool,
    pub blocked: bool,
    pub message: Option<String>,
    pub violations: Vec<LawViolation>,
}

// ============================================================================
// Habit Promotion Protocol
// ============================================================================

/// Habit promotion stage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HabitStage {
    /// Documentation - informal description
    Documentation,
    /// Checklist - explicit steps
    Checklist,
    /// Template - reusable pattern
    Template,
    /// Skill - formalized procedure
    Skill,
    /// Hook - automated enforcement
    Hook,
    /// Hard Gate - non-bypassable
    HardGate,
}

/// Habit promotion record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitPromotionRecord {
    pub habit_id: String,
    pub name: String,
    pub current_stage: HabitStage,
    pub promotion_history: Vec<PromotionEntry>,
    pub determinism_test_passed: bool,
    pub failure_mode_analysis: Option<String>,
    pub rollback_plan: Option<String>,
}

/// Promotion entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionEntry {
    pub from_stage: HabitStage,
    pub to_stage: HabitStage,
    pub promoted_at: DateTime<Utc>,
    pub promoted_by: String,
    pub justification: String,
}

// ============================================================================
// Hooks System Engine
// ============================================================================

/// Main hooks system engine
pub struct HooksSystemEngine {
    system_law: Arc<SystemLawEngine>,
    harness: Arc<HarnessEngineeringEngine>,
    hooks: Arc<RwLock<HashMap<String, HookDefinition>>>,
    event_log: Arc<RwLock<Vec<HookEvent>>>,
    habit_records: Arc<RwLock<HashMap<String, HabitPromotionRecord>>>,
}

impl HooksSystemEngine {
    pub fn new(system_law: Arc<SystemLawEngine>, harness: Arc<HarnessEngineeringEngine>) -> Self {
        Self {
            system_law,
            harness,
            hooks: Arc::new(RwLock::new(HashMap::new())),
            event_log: Arc::new(RwLock::new(Vec::new())),
            habit_records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // ========================================================================
    // Hook Registration
    // ========================================================================

    /// Register a hook
    pub async fn register_hook(&self, hook: HookDefinition) {
        let mut hooks = self.hooks.write().await;
        hooks.insert(hook.hook_id.clone(), hook);
    }

    /// Get hook by ID
    pub async fn get_hook(&self, hook_id: &str) -> Option<HookDefinition> {
        let hooks = self.hooks.read().await;
        hooks.get(hook_id).cloned()
    }

    /// List hooks by layer
    pub async fn list_hooks_by_layer(&self, layer: HookLayer) -> Vec<HookDefinition> {
        let hooks = self.hooks.read().await;
        hooks
            .values()
            .filter(|h| h.layer == layer)
            .cloned()
            .collect()
    }

    /// Enable/disable hook
    pub async fn set_hook_enabled(&self, hook_id: &str, enabled: bool) -> Result<(), HooksError> {
        let mut hooks = self.hooks.write().await;
        let hook = hooks
            .get_mut(hook_id)
            .ok_or_else(|| HooksError::HookNotFound(hook_id.to_string()))?;
        hook.enabled = enabled;
        Ok(())
    }

    // ========================================================================
    // Event Processing
    // ========================================================================

    /// Process hook event
    pub async fn process_event(&self, event: HookEvent) -> Vec<HookResult> {
        // Log event
        {
            let mut event_log = self.event_log.write().await;
            event_log.push(event.clone());
        }

        // Find matching hooks
        let hooks = self.hooks.read().await;
        let mut results = Vec::new();

        for hook in hooks.values() {
            if !hook.enabled {
                continue;
            }

            // Check if hook matches event type
            let event_type = self.get_event_type_name(&event);
            if hook.event_types.contains(&event_type) {
                // Execute hook
                let result = self.execute_hook(hook, &event).await;
                results.push(result);
            }
        }

        results
    }

    /// Get event type name
    fn get_event_type_name(&self, event: &HookEvent) -> String {
        match event {
            HookEvent::Boot(_) => "boot".to_string(),
            HookEvent::ToolInvoke(_) => "tool_invoke".to_string(),
            HookEvent::FileSystem(_) => "file_system".to_string(),
            HookEvent::SpecValidation(_) => "spec_validation".to_string(),
            HookEvent::TaskOutput(_) => "task_output".to_string(),
            HookEvent::Escalation(_) => "escalation".to_string(),
            HookEvent::Approval(_) => "approval".to_string(),
        }
    }

    /// Execute hook
    async fn execute_hook(&self, hook: &HookDefinition, event: &HookEvent) -> HookResult {
        // In production, would call actual handler
        // For now, simulate based on event type

        let mut violations = Vec::new();
        let mut blocked = false;
        let mut message = None;

        match event {
            HookEvent::ToolInvoke(tool_event) => {
                // Check tool safety
                if tool_event.safety_tier == SafetyTier::Critical || tool_event.safety_tier == SafetyTier::Dangerous {
                    if !tool_event.preconditions_met {
                        blocked = true;
                        message = Some("Tool preconditions not met".to_string());
                        
                        // Record SYSTEM_LAW violation
                        let violation_id = self.system_law
                            .record_violation(
                                "LAW-TOOL-001",
                                ViolationSeverity::Hard,
                                "Dangerous tool invoked without preconditions",
                                serde_json::json!({"tool_id": tool_event.tool_id}),
                            )
                            .await;

                        let violations_list = self.system_law.get_unresolved_violations().await;
                        if let Some(violation) = violations_list.into_iter()
                            .find(|v| v.violation_id == violation_id)
                        {
                            violations.push(violation);
                        }
                    }
                }
            }
            HookEvent::SpecValidation(spec_event) => {
                if !spec_event.present {
                    blocked = true;
                    message = Some(format!("Required spec not present: {}", spec_event.spec_type));
                }
                if !spec_event.valid {
                    blocked = true;
                    message = Some(format!("Spec validation failed: {}", spec_event.spec_type));
                }
            }
            _ => {
                // Other events pass by default
            }
        }

        HookResult {
            hook_id: hook.hook_id.clone(),
            success: !blocked,
            blocked,
            message,
            violations,
        }
    }

    // ========================================================================
    // Kernel Hooks
    // ========================================================================

    /// LAW-ENF-001: Boot injection hook
    pub async fn on_boot(&self, phase: &str, component: &str) -> Vec<HookResult> {
        let event = HookEvent::Boot(BootEvent {
            phase: phase.to_string(),
            component: component.to_string(),
            success: true,
            timestamp: Utc::now(),
        });
        self.process_event(event).await
    }

    /// LAW-TOOL-001/002: Tool permission gate
    pub async fn on_tool_invoke(
        &self,
        tool_id: &str,
        tool_type: ToolType,
        safety_tier: SafetyTier,
    ) -> Vec<HookResult> {
        let event = HookEvent::ToolInvoke(ToolInvokeEvent {
            tool_id: tool_id.to_string(),
            tool_type,
            safety_tier,
            preconditions_met: true, // Would check actual preconditions
            timestamp: Utc::now(),
        });
        self.process_event(event).await
    }

    // ========================================================================
    // Workspace Hooks
    // ========================================================================

    /// SYSTEM_LAW validation hook
    pub async fn on_system_law_check(&self) -> Vec<HookResult> {
        let has_violations = self.system_law.has_hard_violations().await;
        
        if has_violations {
            vec![HookResult {
                hook_id: "system_law_check".to_string(),
                success: false,
                blocked: true,
                message: Some("SYSTEM_LAW hard violations present".to_string()),
                violations: self.system_law.get_unresolved_violations().await,
            }]
        } else {
            vec![HookResult {
                hook_id: "system_law_check".to_string(),
                success: true,
                blocked: false,
                message: Some("No SYSTEM_LAW violations".to_string()),
                violations: vec![],
            }]
        }
    }

    /// Spec presence check hook
    pub async fn on_spec_check(&self, spec_type: &str) -> Vec<HookResult> {
        let event = HookEvent::SpecValidation(SpecValidationEvent {
            spec_type: spec_type.to_string(),
            present: true, // Would check actual presence
            valid: true,
            errors: vec![],
            timestamp: Utc::now(),
        });
        self.process_event(event).await
    }

    // ========================================================================
    // Task Hooks
    // ========================================================================

    /// Output schema validation hook
    pub async fn on_task_output(
        &self,
        task_id: &str,
        output_type: &str,
        schema_valid: bool,
    ) -> Vec<HookResult> {
        let event = HookEvent::TaskOutput(TaskOutputEvent {
            task_id: task_id.to_string(),
            output_type: output_type.to_string(),
            schema_valid,
            formatted: true,
            verified: schema_valid,
            timestamp: Utc::now(),
        });
        self.process_event(event).await
    }

    // ========================================================================
    // Human Layer Hooks
    // ========================================================================

    /// Escalation gate hook
    pub async fn on_escalation(
        &self,
        from_level: EscalationLevel,
        to_level: EscalationLevel,
        reason: &str,
    ) -> Vec<HookResult> {
        let event = HookEvent::Escalation(EscalationEvent {
            from_level,
            to_level,
            reason: reason.to_string(),
            timestamp: Utc::now(),
        });
        self.process_event(event).await
    }

    /// Approval trigger hook
    pub async fn on_approval(
        &self,
        approval_type: &str,
        approved: bool,
        approver: &str,
    ) -> Vec<HookResult> {
        let event = HookEvent::Approval(ApprovalEvent {
            approval_type: approval_type.to_string(),
            approved,
            approver: approver.to_string(),
            reason: None,
            timestamp: Utc::now(),
        });
        self.process_event(event).await
    }

    // ========================================================================
    // Habit Promotion Protocol
    // ========================================================================

    /// Register habit for promotion tracking
    pub async fn register_habit(&self, name: &str) -> String {
        let habit_id = format!("habit_{}", Uuid::new_v4().simple());
        
        let record = HabitPromotionRecord {
            habit_id: habit_id.clone(),
            name: name.to_string(),
            current_stage: HabitStage::Documentation,
            promotion_history: vec![],
            determinism_test_passed: false,
            failure_mode_analysis: None,
            rollback_plan: None,
        };

        let mut records = self.habit_records.write().await;
        records.insert(habit_id.clone(), record);

        habit_id
    }

    /// Promote habit to next stage
    pub async fn promote_habit(
        &self,
        habit_id: &str,
        promoted_by: &str,
        justification: &str,
    ) -> Result<HabitStage, HooksError> {
        let mut records = self.habit_records.write().await;
        let record = records
            .get_mut(habit_id)
            .ok_or_else(|| HooksError::HabitNotFound(habit_id.to_string()))?;

        let next_stage = match record.current_stage {
            HabitStage::Documentation => HabitStage::Checklist,
            HabitStage::Checklist => HabitStage::Template,
            HabitStage::Template => HabitStage::Skill,
            HabitStage::Skill => HabitStage::Hook,
            HabitStage::Hook => HabitStage::HardGate,
            HabitStage::HardGate => return Err(HooksError::AlreadyAtHighestStage),
        };

        // Run determinism test for Hook and HardGate stages
        if next_stage == HabitStage::Hook || next_stage == HabitStage::HardGate {
            if !record.determinism_test_passed {
                return Err(HooksError::DeterminismTestFailed);
            }
        }

        let entry = PromotionEntry {
            from_stage: record.current_stage,
            to_stage: next_stage,
            promoted_at: Utc::now(),
            promoted_by: promoted_by.to_string(),
            justification: justification.to_string(),
        };

        record.promotion_history.push(entry);
        record.current_stage = next_stage;

        Ok(next_stage)
    }

    /// Run determinism test for habit
    pub async fn run_determinism_test(&self, habit_id: &str) -> Result<bool, HooksError> {
        let mut records = self.habit_records.write().await;
        let record = records
            .get_mut(habit_id)
            .ok_or_else(|| HooksError::HabitNotFound(habit_id.to_string()))?;

        // In production, would run actual determinism test
        // For now, simulate success
        record.determinism_test_passed = true;

        Ok(true)
    }

    /// Set failure mode analysis
    pub async fn set_failure_mode_analysis(
        &self,
        habit_id: &str,
        analysis: &str,
    ) -> Result<(), HooksError> {
        let mut records = self.habit_records.write().await;
        let record = records
            .get_mut(habit_id)
            .ok_or_else(|| HooksError::HabitNotFound(habit_id.to_string()))?;

        record.failure_mode_analysis = Some(analysis.to_string());
        Ok(())
    }

    /// Set rollback plan
    pub async fn set_rollback_plan(&self, habit_id: &str, plan: &str) -> Result<(), HooksError> {
        let mut records = self.habit_records.write().await;
        let record = records
            .get_mut(habit_id)
            .ok_or_else(|| HooksError::HabitNotFound(habit_id.to_string()))?;

        record.rollback_plan = Some(plan.to_string());
        Ok(())
    }

    /// Get habit record
    pub async fn get_habit_record(&self, habit_id: &str) -> Option<HabitPromotionRecord> {
        let records = self.habit_records.read().await;
        records.get(habit_id).cloned()
    }

    // ========================================================================
    // Event Log Access
    // ========================================================================

    /// Get event log
    pub async fn get_event_log(&self) -> Vec<HookEvent> {
        let event_log = self.event_log.read().await;
        event_log.clone()
    }

    /// Get events by layer
    pub async fn get_events_by_layer(&self, layer: HookLayer) -> Vec<HookEvent> {
        let event_log = self.event_log.read().await;
        event_log
            .iter()
            .filter(|e| match (e, layer) {
                (HookEvent::Boot(_), HookLayer::Kernel) => true,
                (HookEvent::ToolInvoke(_), HookLayer::Kernel) => true,
                (HookEvent::FileSystem(_), HookLayer::Workspace) => true,
                (HookEvent::SpecValidation(_), HookLayer::Workspace) => true,
                (HookEvent::TaskOutput(_), HookLayer::Task) => true,
                (HookEvent::Escalation(_), HookLayer::Human) => true,
                (HookEvent::Approval(_), HookLayer::Human) => true,
                _ => false,
            })
            .cloned()
            .collect()
    }
}

/// Hooks error types
#[derive(Debug, thiserror::Error)]
pub enum HooksError {
    #[error("Hook not found: {0}")]
    HookNotFound(String),

    #[error("Habit not found: {0}")]
    HabitNotFound(String),

    #[error("Already at highest habit stage")]
    AlreadyAtHighestStage,

    #[error("Determinism test failed")]
    DeterminismTestFailed,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_system_law() -> Arc<SystemLawEngine> {
        Arc::new(SystemLawEngine::new())
    }

    fn create_test_harness() -> Arc<HarnessEngineeringEngine> {
        Arc::new(HarnessEngineeringEngine::new(create_test_system_law()))
    }

    #[tokio::test]
    async fn test_register_hook() {
        let engine = HooksSystemEngine::new(create_test_system_law(), create_test_harness());

        let hook = HookDefinition {
            hook_id: "hook_001".to_string(),
            name: "Test Hook".to_string(),
            layer: HookLayer::Kernel,
            event_types: vec!["tool_invoke".to_string()],
            handler: "test_handler".to_string(),
            priority: 1,
            blocking: true,
            enabled: true,
        };

        engine.register_hook(hook).await;

        let retrieved = engine.get_hook("hook_001").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Test Hook");
    }

    #[tokio::test]
    async fn test_tool_invoke_hook() {
        let engine = HooksSystemEngine::new(create_test_system_law(), create_test_harness());

        // Register a hook for tool invocation
        let hook = HookDefinition {
            hook_id: "tool_gate".to_string(),
            name: "Tool Gate".to_string(),
            layer: HookLayer::Kernel,
            event_types: vec!["tool_invoke".to_string()],
            handler: "tool_gate_handler".to_string(),
            priority: 1,
            blocking: true,
            enabled: true,
        };
        engine.register_hook(hook).await;

        // Test safe tool
        let results = engine.on_tool_invoke("fs.read", ToolType::Read, SafetyTier::Safe).await;
        assert!(!results.is_empty());
        assert!(!results[0].blocked);

        // Test dangerous tool without preconditions (simulated failure)
        // In real scenario, would need to mock preconditions_met = false
    }

    #[tokio::test]
    async fn test_system_law_check() {
        let engine = HooksSystemEngine::new(create_test_system_law(), create_test_harness());

        let results = engine.on_system_law_check().await;
        assert!(!results.is_empty());
        assert!(results[0].success); // No violations initially
    }

    #[tokio::test]
    async fn test_habit_promotion() {
        let engine = HooksSystemEngine::new(create_test_system_law(), create_test_harness());

        let habit_id = engine.register_habit("Test Habit").await;

        let record = engine.get_habit_record(&habit_id).await;
        assert!(record.is_some());
        assert_eq!(record.unwrap().current_stage, HabitStage::Documentation);

        // Promote to checklist
        let stage = engine.promote_habit(&habit_id, "test_user", "Initial promotion").await;
        assert!(stage.is_ok());
        assert_eq!(stage.unwrap(), HabitStage::Checklist);

        // Run determinism test
        let test_result = engine.run_determinism_test(&habit_id).await;
        assert!(test_result.is_ok());
        assert!(test_result.unwrap());

        // Promote all the way to Hook
        for _ in 0..3 {
            engine.promote_habit(&habit_id, "test_user", "Promotion").await.unwrap();
        }

        let record = engine.get_habit_record(&habit_id).await.unwrap();
        assert_eq!(record.current_stage, HabitStage::Hook);
    }

    #[tokio::test]
    async fn test_event_logging() {
        let engine = HooksSystemEngine::new(create_test_system_law(), create_test_harness());

        // Trigger some events
        engine.on_boot("init", "kernel").await;
        engine.on_tool_invoke("fs.read", ToolType::Read, SafetyTier::Safe).await;

        let events = engine.get_event_log().await;
        assert_eq!(events.len(), 2);

        let kernel_events = engine.get_events_by_layer(HookLayer::Kernel).await;
        assert_eq!(kernel_events.len(), 2);
    }

}
