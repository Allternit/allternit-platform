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
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelABI {
    pub version: String,
    pub abi_hash: String, // SHA256 of the ABI
    pub timestamp: u64,
    pub components: HashMap<String, ComponentABI>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentABI {
    pub name: String,
    pub version: String,
    pub hash: String, // SHA256 of the component interface
    pub dependencies: Vec<String>,
    pub invariants: Vec<String>, // Behavioral invariants
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub report_id: String,
    pub timestamp: u64,
    pub baseline_abi: KernelABI,
    pub current_abi: KernelABI,
    pub compatibility_issues: Vec<CompatibilityIssue>,
    pub breaking_changes: Vec<BreakingChange>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityIssue {
    pub component_name: String,
    pub issue_type: IssueType,
    pub severity: Severity,
    pub description: String,
    pub suggested_fix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IssueType {
    ABIChange,
    InterfaceChange,
    BehaviorDrift,
    DependencyConflict,
    InvariantViolation,
    SecurityRegression,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakingChange {
    pub component_name: String,
    pub change_type: BreakingChangeType,
    pub description: String,
    pub affected_contracts: Vec<String>,
    pub migration_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BreakingChangeType {
    RemovedInterface,
    ChangedSignature,
    RemovedField,
    ChangedFieldType,
    RemovedEnumVariant,
    ChangedSemantics,
}

#[derive(Debug, thiserror::Error)]
pub enum CompatibilityError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] HistoryError),
    #[error("ABI mismatch: {0}")]
    ABIMismatch(String),
    #[error("Breaking change detected: {0}")]
    BreakingChangeDetected(String),
    #[error("Invariant violated: {0}")]
    InvariantViolated(String),
    #[error("Version mismatch: {0}")]
    VersionMismatch(String),
    #[error("Mutex poisoned: {0}")]
    MutexPoisoned(String),
    #[error("Time error: {0}")]
    TimeError(String),
}

pub struct KernelCompatibilityChecker {
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

    // Cached ABIs for comparison
    baseline_abi: Arc<RwLock<Option<KernelABI>>>,
    current_abi: Arc<RwLock<Option<KernelABI>>>,
}

impl KernelCompatibilityChecker {
    pub fn new(
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
        KernelCompatibilityChecker {
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
            baseline_abi: Arc::new(RwLock::new(None)),
            current_abi: Arc::new(RwLock::new(None)),
        }
    }

    /// Capture the current ABI state as a baseline
    pub async fn capture_baseline(&self) -> Result<String, CompatibilityError> {
        let abi = self.generate_current_abi().await?;

        {
            let mut baseline_guard = self.baseline_abi.write().await;
            *baseline_guard = Some(abi.clone());
        }

        // Log the baseline capture
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "KernelABIBaselineCaptured".to_string(),
            session_id: "system".to_string(),
            tenant_id: "system".to_string(),
            actor_id: "kernel_compat_checker".to_string(),
            role: "system".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| CompatibilityError::TimeError(e.to_string()))?
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "baseline_version": abi.version,
                "abi_hash": abi.abi_hash,
                "timestamp": abi.timestamp,
                "component_count": abi.components.len(),
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock()
                .map_err(|e| CompatibilityError::MutexPoisoned(e.to_string()))?;
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

        Ok(abi.version)
    }

    /// Generate the current ABI from all components
    pub async fn generate_current_abi(&self) -> Result<KernelABI, CompatibilityError> {
        let mut components = HashMap::new();

        // Add ABI for each component
        components.insert("history".to_string(), self.get_history_abi().await?);
        components.insert("messaging".to_string(), self.get_messaging_abi().await?);
        components.insert("policy".to_string(), self.get_policy_abi().await?);
        components.insert(
            "context_router".to_string(),
            self.get_context_router_abi().await?,
        );
        components.insert("memory".to_string(), self.get_memory_abi().await?);
        components.insert("providers".to_string(), self.get_providers_abi().await?);
        components.insert("skills".to_string(), self.get_skills_abi().await?);
        components.insert("workflows".to_string(), self.get_workflows_abi().await?);
        components.insert("embodiment".to_string(), self.get_embodiment_abi().await?);
        components.insert("packaging".to_string(), self.get_packaging_abi().await?);
        components.insert("evals".to_string(), self.get_evals_abi().await?);
        components.insert(
            "runtime_core".to_string(),
            self.get_runtime_core_abi().await?,
        );

        // Calculate overall ABI hash
        let mut hasher = sha2::Sha256::new();
        for (name, component_abi) in &components {
            hasher.update(format!("{}:{}", name, component_abi.hash).as_bytes());
        }
        let abi_hash = format!("{:x}", hasher.finalize());

        let abi = KernelABI {
            version: format!(
                "v{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| CompatibilityError::TimeError(e.to_string()))?
                    .as_secs()
            ),
            abi_hash,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| CompatibilityError::TimeError(e.to_string()))?
                .as_secs(),
            components,
        };

        // Cache the current ABI
        {
            let mut current_guard = self.current_abi.write().await;
            *current_guard = Some(abi.clone());
        }

        Ok(abi)
    }

    /// Check compatibility between baseline and current ABI
    pub async fn check_compatibility(&self) -> Result<CompatibilityReport, CompatibilityError> {
        let baseline = {
            let baseline_guard = self.baseline_abi.read().await;
            baseline_guard.clone().ok_or_else(|| {
                CompatibilityError::ABIMismatch("No baseline ABI captured".to_string())
            })?
        };

        let current = self.generate_current_abi().await?;

        let mut issues = Vec::new();
        let mut breaking_changes = Vec::new();
        let mut recommendations = Vec::new();

        // Compare components
        for (name, current_component) in &current.components {
            if let Some(baseline_component) = baseline.components.get(name) {
                // Check if the component ABI has changed
                if current_component.hash != baseline_component.hash {
                    let issue = CompatibilityIssue {
                        component_name: name.clone(),
                        issue_type: IssueType::ABIChange,
                        severity: Severity::Critical,
                        description: format!(
                            "Component {} ABI changed from {} to {}",
                            name, baseline_component.hash, current_component.hash
                        ),
                        suggested_fix: Some(
                            "Review the changes and update version if intentional".to_string(),
                        ),
                    };

                    issues.push(issue);

                    // Check if this is a breaking change
                    if self.is_breaking_change(baseline_component, current_component) {
                        let breaking_change = BreakingChange {
                            component_name: name.clone(),
                            change_type: BreakingChangeType::ChangedSignature,
                            description: format!("Breaking change detected in component {}", name),
                            affected_contracts: vec![format!("{}::interface", name)],
                            migration_path: Some(
                                "Update client code to match new interface".to_string(),
                            ),
                        };

                        breaking_changes.push(breaking_change);
                    }
                }
            } else {
                // New component added
                let issue = CompatibilityIssue {
                    component_name: name.clone(),
                    issue_type: IssueType::ABIChange,
                    severity: Severity::Warning,
                    description: format!("New component {} added to ABI", name),
                    suggested_fix: Some("Document the new component and its interface".to_string()),
                };

                issues.push(issue);
            }
        }

        // Check for removed components
        for (name, baseline_component) in &baseline.components {
            if !current.components.contains_key(name) {
                let issue = CompatibilityIssue {
                    component_name: name.clone(),
                    issue_type: IssueType::ABIChange,
                    severity: Severity::Critical,
                    description: format!("Component {} was removed from ABI", name),
                    suggested_fix: Some(
                        "Re-add the component or update dependent code".to_string(),
                    ),
                };

                issues.push(issue);

                let breaking_change = BreakingChange {
                    component_name: name.clone(),
                    change_type: BreakingChangeType::RemovedInterface,
                    description: format!("Component {} was removed", name),
                    affected_contracts: vec![format!("{}::interface", name)],
                    migration_path: Some(
                        "Update dependent code to not rely on removed component".to_string(),
                    ),
                };

                breaking_changes.push(breaking_change);
            }
        }

        // Generate recommendations
        if !breaking_changes.is_empty() {
            recommendations.push("Breaking changes detected - version bump required".to_string());
        }

        if !issues.is_empty() {
            recommendations.push("ABI changes detected - review compatibility".to_string());
        }

        let report = CompatibilityReport {
            report_id: Uuid::new_v4().to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| CompatibilityError::TimeError(e.to_string()))?
                .as_secs(),
            baseline_abi: baseline,
            current_abi: current,
            compatibility_issues: issues,
            breaking_changes,
            recommendations,
        };

        // Log the compatibility report
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "KernelABICompatibilityReport".to_string(),
            session_id: "system".to_string(),
            tenant_id: "system".to_string(),
            actor_id: "kernel_compat_checker".to_string(),
            role: "system".to_string(),
            timestamp: report.timestamp,
            trace_id: None,
            payload: serde_json::json!({
                "report_id": report.report_id,
                "issue_count": report.compatibility_issues.len(),
                "breaking_change_count": report.breaking_changes.len(),
                "has_breaking_changes": !report.breaking_changes.is_empty(),
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock()
                .map_err(|e| CompatibilityError::MutexPoisoned(e.to_string()))?;
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

        Ok(report)
    }

    /// Check if a component change is breaking
    fn is_breaking_change(&self, baseline: &ComponentABI, current: &ComponentABI) -> bool {
        // For now, any change in hash is considered potentially breaking
        // In a real implementation, we would do more sophisticated analysis
        baseline.hash != current.hash
    }

    /// Validate that the current system matches the expected ABI version
    pub async fn validate_version(
        &self,
        expected_version: &str,
    ) -> Result<bool, CompatibilityError> {
        let current_abi = self.generate_current_abi().await?;

        if current_abi.version.starts_with(expected_version) {
            Ok(true)
        } else {
            Err(CompatibilityError::VersionMismatch(format!(
                "Expected version {}, got {}",
                expected_version, current_abi.version
            )))
        }
    }

    // Helper methods to get ABI for each component
    async fn get_history_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        // In a real implementation, this would analyze the actual interface
        // For now, we'll return a placeholder
        Ok(ComponentABI {
            name: "history".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "history_abi_placeholder_hash".to_string(),
            dependencies: vec!["serde".to_string(), "tokio".to_string()],
            invariants: vec!["append-only".to_string(), "immutable_entries".to_string()],
        })
    }

    async fn get_messaging_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "messaging".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "messaging_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
            ],
            invariants: vec![
                "durable_delivery".to_string(),
                "ordered_processing".to_string(),
            ],
        })
    }

    async fn get_policy_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "policy".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "policy_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
            ],
            invariants: vec![
                "centralized_decision_making".to_string(),
                "audit_trail".to_string(),
            ],
        })
    }

    async fn get_context_router_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "context_router".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "context_router_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
            ],
            invariants: vec![
                "context_preservation".to_string(),
                "access_control".to_string(),
            ],
        })
    }

    async fn get_memory_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "memory".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "memory_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "context_router".to_string(),
            ],
            invariants: vec!["consistency".to_string(), "retention_policies".to_string()],
        })
    }

    async fn get_providers_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "providers".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "providers_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "context_router".to_string(),
                "memory".to_string(),
            ],
            invariants: vec!["provider_routing".to_string(), "persona_kernel".to_string()],
        })
    }

    async fn get_skills_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "skills".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "skills_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "messaging".to_string(),
            ],
            invariants: vec![
                "signed_verification".to_string(),
                "capability_based".to_string(),
            ],
        })
    }

    async fn get_workflows_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "workflows".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "workflows_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "messaging".to_string(),
                "skills".to_string(),
            ],
            invariants: vec![
                "state_preservation".to_string(),
                "recoverable_execution".to_string(),
            ],
        })
    }

    async fn get_embodiment_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "embodiment".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "embodiment_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "messaging".to_string(),
                "providers".to_string(),
                "skills".to_string(),
                "workflows".to_string(),
                "context_router".to_string(),
                "memory".to_string(),
            ],
            invariants: vec!["safe_execution".to_string(), "simulation_first".to_string()],
        })
    }

    async fn get_packaging_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "packaging".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "packaging_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "messaging".to_string(),
                "providers".to_string(),
                "skills".to_string(),
                "workflows".to_string(),
                "embodiment".to_string(),
                "context_router".to_string(),
                "memory".to_string(),
            ],
            invariants: vec![
                "secure_deployment".to_string(),
                "compliance_checking".to_string(),
            ],
        })
    }

    async fn get_evals_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "evals".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "evals_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "policy".to_string(),
                "messaging".to_string(),
                "providers".to_string(),
                "skills".to_string(),
                "workflows".to_string(),
                "embodiment".to_string(),
                "packaging".to_string(),
                "context_router".to_string(),
                "memory".to_string(),
            ],
            invariants: vec![
                "repeatable_tests".to_string(),
                "objective_metrics".to_string(),
            ],
        })
    }

    async fn get_runtime_core_abi(&self) -> Result<ComponentABI, CompatibilityError> {
        Ok(ComponentABI {
            name: "runtime_core".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hash: "runtime_core_abi_placeholder_hash".to_string(),
            dependencies: vec![
                "serde".to_string(),
                "tokio".to_string(),
                "history".to_string(),
                "messaging".to_string(),
            ],
            invariants: vec![
                "deterministic_execution".to_string(),
                "replay_capability".to_string(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_kernel_compatibility_checker() {
        // Create temporary database
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();

        // Create temporary history ledger
        let temp_path = format!("/tmp/test_kernel_compat_{}.jsonl", Uuid::new_v4());
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

        // Create skill registry
        let tool_gateway = Arc::new(allternit_tools_gateway::ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));

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

        // Create workflow engine
        // Create task queue
        let task_queue = Arc::new(
            allternit_messaging::TaskQueue::new(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );

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

        // Create kernel compatibility checker
        let compat_checker = Arc::new(KernelCompatibilityChecker::new(
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

        // Capture baseline ABI
        let baseline_version = compat_checker.capture_baseline().await.unwrap();
        assert!(!baseline_version.is_empty());

        // Generate current ABI
        let current_abi = compat_checker.generate_current_abi().await.unwrap();
        assert!(!current_abi.abi_hash.is_empty());

        // Check compatibility (should be compatible since it's the same baseline)
        let report = compat_checker.check_compatibility().await.unwrap();
        assert_eq!(report.compatibility_issues.len(), 0);
        assert_eq!(report.breaking_changes.len(), 0);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
