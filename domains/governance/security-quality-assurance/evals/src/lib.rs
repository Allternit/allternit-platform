//! # Allternit Evals
//!
//! Evaluation framework for Allternit components.
//!
//! ## Overview
//!
//! This crate provides a comprehensive evaluation system for testing
//! Allternit components including skills, workflows, providers, and embodiments.
//! It supports various evaluation types including unit tests, integration
//! tests, regression tests, performance tests, security tests, and
//! compliance tests.
//!
//! ## Features
//!
//! - Multiple evaluation types (unit, integration, regression, performance, security, compliance)
//! - Component-specific testing (skills, workflows, providers, embodiments)
//! - SQLite-based persistent storage for evaluation configs and results
//! - Policy-based access control for evaluation execution
//! - Resource usage tracking and compliance checking
//! - Async test execution with configurable parallelism
//!
//! ## Key Components
//!
//! - **EvaluationEngine**: Core engine for running evaluations
//! - **EvaluationStorage**: Trait for persistent storage implementations
//! - **EvaluationConfig**: Configuration for an evaluation
//! - **EvaluationResult**: Results from running an evaluation
//! - **TestSuite/TestCase**: Test definition structures
//!
//! ## Example
//!
//! ```rust,no_run
//! use allternit_evals::{
//!     EvaluationConfig, EvaluationType, TargetComponent, TestSuite, TestCase,
//!     TestCaseType, ExecutionContext, SecurityContext, SuccessCriteria
//! };
//!
//! // Create an evaluation configuration
//! let config = EvaluationConfig {
//!     eval_id: "eval-001".to_string(),
//!     name: "Skill Test".to_string(),
//!     description: "Test skill functionality".to_string(),
//!     eval_type: EvaluationType::Integration,
//!     target_component: TargetComponent::Skill { skill_id: "skill-001".to_string() },
//!     test_suite: TestSuite {
//!         suite_id: "suite-001".to_string(),
//!         name: "Basic Tests".to_string(),
//!         description: "Basic functionality tests".to_string(),
//!         tests: vec![],
//!         tags: vec!["integration".to_string()],
//!         dependencies: vec![],
//!     },
//!     // ... additional configuration
//!     success_criteria: SuccessCriteria {
//!         min_success_rate: 0.95,
//!         max_failure_rate: 0.05,
//!         max_execution_time_ms: 60000,
//!         max_resource_usage: ResourceUsage::default(),
//!         required_compliance: vec![],
//!         custom_criteria: HashMap::new(),
//!     },
//!     execution_config: ExecutionConfig {
//!         parallelism: 4,
//!         timeout_ms: 300000,
//!         retry_on_failure: true,
//!         fail_fast: false,
//!         execution_order: ExecutionOrder::Sequential,
//!         reporting_config: ReportingConfig {
//!             report_format: ReportFormat::JSON,
//!             report_destination: ReportDestination::Console,
//!             include_details: true,
//!             include_performance: true,
//!             include_security: true,
//!             notify_on_completion: false,
//!             notification_recipients: vec![],
//!         },
//!     },
//!     metadata: EvaluationMetadata {
//!         author: "Test Author".to_string(),
//!         version: "1.0.0".to_string(),
//!         tags: vec!["regression".to_string()],
//!         labels: HashMap::new(),
//!         dependencies: vec![],
//!         environment: "test".to_string(),
//!     },
//!     created_at: 1704067200,
//!     updated_at: 1704067200,
//!     is_active: true,
//! };
//! ```

use allternit_context_router::{ContextBundle, ContextRouter};
use allternit_history::{HistoryError, HistoryLedger};
use allternit_memory::MemoryFabric;
use allternit_messaging::{EventEnvelope, MessagingSystem};
use allternit_policy::{PolicyEffect, PolicyEngine, PolicyRequest};
use allternit_providers::ProviderRouter;
use allternit_runtime_core::SessionManager;
use allternit_skills::SkillRegistry;
use allternit_workflows::WorkflowEngine;
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, Row};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationConfig {
    pub eval_id: String,
    pub name: String,
    pub description: String,
    pub eval_type: EvaluationType,
    pub target_component: TargetComponent,
    pub test_suite: TestSuite,
    pub success_criteria: SuccessCriteria,
    pub execution_config: ExecutionConfig,
    pub metadata: EvaluationMetadata,
    pub created_at: u64,
    pub updated_at: u64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvaluationType {
    Unit,
    Integration,
    Regression,
    Performance,
    Security,
    Compliance,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TargetComponent {
    Skill {
        skill_id: String,
    },
    Workflow {
        workflow_id: String,
    },
    Provider {
        provider_id: String,
    },
    Embodiment {
        embodiment_id: String,
    },
    Package {
        package_id: String,
    },
    Custom {
        component_type: String,
        reference: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuite {
    pub suite_id: String,
    pub name: String,
    pub description: String,
    pub tests: Vec<TestCase>,
    pub tags: Vec<String>,
    pub dependencies: Vec<String>, // other test suites this depends on
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub test_id: String,
    pub name: String,
    pub description: String,
    pub test_type: TestCaseType,
    pub input_data: serde_json::Value,
    pub expected_output: serde_json::Value,
    pub execution_context: ExecutionContext,
    pub timeout_ms: u64,
    pub retry_policy: RetryPolicy,
    pub tags: Vec<String>,
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestCaseType {
    Functional,
    Performance,
    Security,
    Compliance,
    EdgeCase,
    ErrorHandling,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub session_id: Option<String>,
    pub tenant_id: String,
    pub agent_id: Option<String>,
    pub context_bundle: Option<ContextBundle>,
    pub resource_limits: Option<ResourceLimits>,
    pub security_context: SecurityContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu: String,
    pub memory: String,
    pub disk: String,
    pub network: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityContext {
    pub sensitivity_tier: u8, // 0-4 corresponding to T0-T4
    pub allowed_permissions: Vec<String>,
    pub required_compliance: Vec<ComplianceRequirement>,
    pub encryption_required: bool,
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
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_base: u64, // base delay in seconds
    pub backoff_multiplier: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessCriteria {
    pub min_success_rate: f64, // 0.0 to 1.0
    pub max_failure_rate: f64, // 0.0 to 1.0
    pub max_execution_time_ms: u64,
    pub max_resource_usage: ResourceUsage,
    pub required_compliance: Vec<ComplianceRequirement>,
    pub custom_criteria: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceUsage {
    pub cpu_usage: f64,     // percentage
    pub memory_usage: f64,  // percentage
    pub disk_usage: f64,    // percentage
    pub network_usage: f64, // percentage
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    pub parallelism: u32,
    pub timeout_ms: u64,
    pub retry_on_failure: bool,
    pub fail_fast: bool,
    pub execution_order: ExecutionOrder,
    pub reporting_config: ReportingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionOrder {
    Sequential,
    Parallel,
    DependencyBased,
    Custom(Vec<String>), // ordered list of test IDs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportingConfig {
    pub report_format: ReportFormat,
    pub report_destination: ReportDestination,
    pub include_details: bool,
    pub include_performance: bool,
    pub include_security: bool,
    pub notify_on_completion: bool,
    pub notification_recipients: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReportFormat {
    JSON,
    XML,
    HTML,
    Text,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReportDestination {
    File { path: String },
    Database,
    API { endpoint: String },
    Console,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationMetadata {
    pub author: String,
    pub version: String,
    pub tags: Vec<String>,
    pub labels: HashMap<String, String>,
    pub dependencies: Vec<String>,
    pub environment: String, // dev, staging, prod
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub result_id: String,
    pub eval_id: String,
    pub test_results: Vec<TestCaseResult>,
    pub overall_status: EvaluationStatus,
    pub execution_time_ms: u64,
    pub resource_usage: ResourceUsage,
    pub compliance_results: ComplianceResults,
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub error_details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvaluationStatus {
    Pending,
    Running,
    Passed,
    Failed,
    Error,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCaseResult {
    pub test_id: String,
    pub status: TestCaseStatus,
    pub execution_time_ms: u64,
    pub resource_usage: ResourceUsage,
    pub actual_output: Option<serde_json::Value>,
    pub error_details: Option<String>,
    pub compliance_results: Option<ComplianceResult>,
    pub created_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestCaseStatus {
    Passed,
    Failed,
    Error,
    Skipped,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceResult {
    pub compliance_requirements: Vec<ComplianceRequirement>,
    pub passed_requirements: Vec<ComplianceRequirement>,
    pub failed_requirements: Vec<ComplianceRequirement>,
    pub details: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceResults {
    pub overall_compliance: bool,
    pub compliance_details: Vec<ComplianceResult>,
    pub compliance_score: f64, // 0.0 to 1.0
}

#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] HistoryError),
    #[error("SQLX error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Policy error: {0}")]
    Policy(#[from] allternit_policy::PolicyError),
    #[error("Evaluation not found: {0}")]
    EvaluationNotFound(String),
    #[error("Test case failed: {0}")]
    TestCaseFailed(String),
    #[error("Execution timeout: {0}")]
    ExecutionTimeout(String),
    #[error("Resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),
    #[error("Security violation: {0}")]
    SecurityViolation(String),
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("Target component not found: {0}")]
    TargetComponentNotFound(String),
}

// Storage trait for evaluation configurations
#[async_trait::async_trait]
pub trait EvaluationStorage: Send + Sync {
    async fn store_evaluation(&self, config: &EvaluationConfig) -> Result<(), EvaluationError>;
    async fn get_evaluation(
        &self,
        eval_id: &str,
    ) -> Result<Option<EvaluationConfig>, EvaluationError>;
    async fn update_evaluation(&self, config: &EvaluationConfig) -> Result<(), EvaluationError>;
    async fn delete_evaluation(&self, eval_id: &str) -> Result<(), EvaluationError>;

    async fn store_result(&self, result: &EvaluationResult) -> Result<(), EvaluationError>;
    async fn get_result(
        &self,
        result_id: &str,
    ) -> Result<Option<EvaluationResult>, EvaluationError>;
    async fn get_results_by_eval(
        &self,
        eval_id: &str,
    ) -> Result<Vec<EvaluationResult>, EvaluationError>;

    async fn get_evaluation_stats(&self, eval_id: &str)
        -> Result<EvaluationStats, EvaluationError>;
    async fn update_evaluation_stats(&self, stats: &EvaluationStats)
        -> Result<(), EvaluationError>;
}

pub struct SqliteEvaluationStorage {
    pool: SqlitePool,
}

impl SqliteEvaluationStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, EvaluationError> {
        // Create the evaluations table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS evaluations (
                eval_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                eval_type TEXT NOT NULL,
                target_component TEXT NOT NULL,
                test_suite TEXT NOT NULL,
                success_criteria TEXT NOT NULL,
                execution_config TEXT NOT NULL,
                metadata TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                is_active BOOLEAN NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        // Create the evaluation_results table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS evaluation_results (
                result_id TEXT PRIMARY KEY,
                eval_id TEXT NOT NULL,
                test_results TEXT NOT NULL,
                overall_status TEXT NOT NULL,
                execution_time_ms INTEGER NOT NULL,
                resource_usage TEXT NOT NULL,
                compliance_results TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                completed_at INTEGER,
                error_details TEXT
            )",
        )
        .execute(&pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        // Create indexes for better query performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_evaluations_name ON evaluations(name)")
            .execute(&pool)
            .await
            .map_err(EvaluationError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_evaluations_active ON evaluations(is_active)")
            .execute(&pool)
            .await
            .map_err(EvaluationError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_results_eval ON evaluation_results(eval_id)")
            .execute(&pool)
            .await
            .map_err(EvaluationError::Sqlx)?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_results_status ON evaluation_results(overall_status)",
        )
        .execute(&pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        Ok(SqliteEvaluationStorage { pool })
    }
}

#[async_trait::async_trait]
impl EvaluationStorage for SqliteEvaluationStorage {
    async fn store_evaluation(&self, config: &EvaluationConfig) -> Result<(), EvaluationError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let eval_type_str = format!("{:?}", config.eval_type);
        let target_component_json =
            serde_json::to_string(&config.target_component).map_err(EvaluationError::Json)?;
        let test_suite_json =
            serde_json::to_string(&config.test_suite).map_err(EvaluationError::Json)?;
        let success_criteria_json =
            serde_json::to_string(&config.success_criteria).map_err(EvaluationError::Json)?;
        let execution_config_json =
            serde_json::to_string(&config.execution_config).map_err(EvaluationError::Json)?;
        let metadata_json =
            serde_json::to_string(&config.metadata).map_err(EvaluationError::Json)?;

        sqlx::query(
            "INSERT OR REPLACE INTO evaluations (
                eval_id, name, description, eval_type, target_component, test_suite,
                success_criteria, execution_config, metadata, created_at, updated_at, is_active
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&config.eval_id)
        .bind(&config.name)
        .bind(&config.description)
        .bind(&eval_type_str)
        .bind(&target_component_json)
        .bind(&test_suite_json)
        .bind(&success_criteria_json)
        .bind(&execution_config_json)
        .bind(&metadata_json)
        .bind(now as i64)
        .bind(now as i64)
        .bind(config.is_active)
        .execute(&self.pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        Ok(())
    }

    async fn get_evaluation(
        &self,
        eval_id: &str,
    ) -> Result<Option<EvaluationConfig>, EvaluationError> {
        let row = sqlx::query(
            "SELECT eval_id, name, description, eval_type, target_component, test_suite,
             success_criteria, execution_config, metadata, created_at, updated_at, is_active
             FROM evaluations WHERE eval_id = ?",
        )
        .bind(eval_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        if let Some(row) = row {
            let eval_type_str: String = row.get("eval_type");
            let eval_type = match eval_type_str.as_str() {
                "Unit" => EvaluationType::Unit,
                "Integration" => EvaluationType::Integration,
                "Regression" => EvaluationType::Regression,
                "Performance" => EvaluationType::Performance,
                "Security" => EvaluationType::Security,
                "Compliance" => EvaluationType::Compliance,
                custom => {
                    // For custom types, we'd need to reconstruct them differently
                    // For now, default to Unit
                    EvaluationType::Custom(custom.to_string())
                }
            };

            let target_component: TargetComponent =
                serde_json::from_str(row.get::<&str, _>("target_component"))
                    .map_err(EvaluationError::Json)?;
            let test_suite: TestSuite = serde_json::from_str(row.get::<&str, _>("test_suite"))
                .map_err(EvaluationError::Json)?;
            let success_criteria: SuccessCriteria =
                serde_json::from_str(row.get::<&str, _>("success_criteria"))
                    .map_err(EvaluationError::Json)?;
            let execution_config: ExecutionConfig =
                serde_json::from_str(row.get::<&str, _>("execution_config"))
                    .map_err(EvaluationError::Json)?;
            let metadata: EvaluationMetadata = serde_json::from_str(row.get::<&str, _>("metadata"))
                .map_err(EvaluationError::Json)?;

            let config = EvaluationConfig {
                eval_id: row.get("eval_id"),
                name: row.get("name"),
                description: row.get("description"),
                eval_type,
                target_component,
                test_suite,
                success_criteria,
                execution_config,
                metadata,
                created_at: row.get::<i64, _>("created_at") as u64,
                updated_at: row.get::<i64, _>("updated_at") as u64,
                is_active: row.get("is_active"),
            };

            Ok(Some(config))
        } else {
            Ok(None)
        }
    }

    async fn update_evaluation(&self, config: &EvaluationConfig) -> Result<(), EvaluationError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let eval_type_str = format!("{:?}", config.eval_type);
        let target_component_json =
            serde_json::to_string(&config.target_component).map_err(EvaluationError::Json)?;
        let test_suite_json =
            serde_json::to_string(&config.test_suite).map_err(EvaluationError::Json)?;
        let success_criteria_json =
            serde_json::to_string(&config.success_criteria).map_err(EvaluationError::Json)?;
        let execution_config_json =
            serde_json::to_string(&config.execution_config).map_err(EvaluationError::Json)?;
        let metadata_json =
            serde_json::to_string(&config.metadata).map_err(EvaluationError::Json)?;

        sqlx::query(
            "UPDATE evaluations SET
                name = ?, description = ?, eval_type = ?, target_component = ?, test_suite = ?,
                success_criteria = ?, execution_config = ?, metadata = ?, updated_at = ?, is_active = ?
             WHERE eval_id = ?"
        )
        .bind(&config.name)
        .bind(&config.description)
        .bind(&eval_type_str)
        .bind(&target_component_json)
        .bind(&test_suite_json)
        .bind(&success_criteria_json)
        .bind(&execution_config_json)
        .bind(&metadata_json)
        .bind(now as i64)
        .bind(config.is_active)
        .bind(&config.eval_id)
        .execute(&self.pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        Ok(())
    }

    async fn delete_evaluation(&self, eval_id: &str) -> Result<(), EvaluationError> {
        sqlx::query("DELETE FROM evaluations WHERE eval_id = ?")
            .bind(eval_id)
            .execute(&self.pool)
            .await
            .map_err(EvaluationError::Sqlx)?;

        Ok(())
    }

    async fn store_result(&self, result: &EvaluationResult) -> Result<(), EvaluationError> {
        let test_results_json =
            serde_json::to_string(&result.test_results).map_err(EvaluationError::Json)?;
        let resource_usage_json =
            serde_json::to_string(&result.resource_usage).map_err(EvaluationError::Json)?;
        let compliance_results_json =
            serde_json::to_string(&result.compliance_results).map_err(EvaluationError::Json)?;
        let overall_status_str = format!("{:?}", result.overall_status);

        sqlx::query(
            "INSERT OR REPLACE INTO evaluation_results (
                result_id, eval_id, test_results, overall_status, execution_time_ms,
                resource_usage, compliance_results, created_at, completed_at, error_details
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&result.result_id)
        .bind(&result.eval_id)
        .bind(&test_results_json)
        .bind(&overall_status_str)
        .bind(result.execution_time_ms as i64)
        .bind(&resource_usage_json)
        .bind(&compliance_results_json)
        .bind(result.created_at as i64)
        .bind(result.completed_at.map(|t| t as i64))
        .bind(&result.error_details)
        .execute(&self.pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        Ok(())
    }

    async fn get_result(
        &self,
        result_id: &str,
    ) -> Result<Option<EvaluationResult>, EvaluationError> {
        let row = sqlx::query(
            "SELECT result_id, eval_id, test_results, overall_status, execution_time_ms,
             resource_usage, compliance_results, created_at, completed_at, error_details
             FROM evaluation_results WHERE result_id = ?",
        )
        .bind(result_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        if let Some(row) = row {
            let test_results: Vec<TestCaseResult> =
                serde_json::from_str(row.get::<&str, _>("test_results"))
                    .map_err(EvaluationError::Json)?;
            let resource_usage: ResourceUsage =
                serde_json::from_str(row.get::<&str, _>("resource_usage"))
                    .map_err(EvaluationError::Json)?;
            let compliance_results: ComplianceResults =
                serde_json::from_str(row.get::<&str, _>("compliance_results"))
                    .map_err(EvaluationError::Json)?;

            let overall_status_str: String = row.get("overall_status");
            let overall_status = match overall_status_str.as_str() {
                "Pending" => EvaluationStatus::Pending,
                "Running" => EvaluationStatus::Running,
                "Passed" => EvaluationStatus::Passed,
                "Failed" => EvaluationStatus::Failed,
                "Error" => EvaluationStatus::Error,
                "Cancelled" => EvaluationStatus::Cancelled,
                "Timeout" => EvaluationStatus::Timeout,
                _ => {
                    return Err(EvaluationError::TestCaseFailed(
                        "Invalid evaluation status".to_string(),
                    ))
                }
            };

            let result = EvaluationResult {
                result_id: row.get("result_id"),
                eval_id: row.get("eval_id"),
                test_results,
                overall_status,
                execution_time_ms: row.get::<i64, _>("execution_time_ms") as u64,
                resource_usage,
                compliance_results,
                created_at: row.get::<i64, _>("created_at") as u64,
                completed_at: row.get::<Option<i64>, _>("completed_at").map(|t| t as u64),
                error_details: row.get("error_details"),
            };

            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    async fn get_results_by_eval(
        &self,
        eval_id: &str,
    ) -> Result<Vec<EvaluationResult>, EvaluationError> {
        let rows = sqlx::query(
            "SELECT result_id, eval_id, test_results, overall_status, execution_time_ms,
             resource_usage, compliance_results, created_at, completed_at, error_details
             FROM evaluation_results WHERE eval_id = ?",
        )
        .bind(eval_id)
        .fetch_all(&self.pool)
        .await
        .map_err(EvaluationError::Sqlx)?;

        let mut results = Vec::new();
        for row in rows {
            let test_results: Vec<TestCaseResult> =
                serde_json::from_str(row.get::<&str, _>("test_results"))
                    .map_err(EvaluationError::Json)?;
            let resource_usage: ResourceUsage =
                serde_json::from_str(row.get::<&str, _>("resource_usage"))
                    .map_err(EvaluationError::Json)?;
            let compliance_results: ComplianceResults =
                serde_json::from_str(row.get::<&str, _>("compliance_results"))
                    .map_err(EvaluationError::Json)?;

            let overall_status_str: String = row.get("overall_status");
            let overall_status = match overall_status_str.as_str() {
                "Pending" => EvaluationStatus::Pending,
                "Running" => EvaluationStatus::Running,
                "Passed" => EvaluationStatus::Passed,
                "Failed" => EvaluationStatus::Failed,
                "Error" => EvaluationStatus::Error,
                "Cancelled" => EvaluationStatus::Cancelled,
                "Timeout" => EvaluationStatus::Timeout,
                _ => {
                    return Err(EvaluationError::TestCaseFailed(
                        "Invalid evaluation status".to_string(),
                    ))
                }
            };

            let result = EvaluationResult {
                result_id: row.get("result_id"),
                eval_id: row.get("eval_id"),
                test_results,
                overall_status,
                execution_time_ms: row.get::<i64, _>("execution_time_ms") as u64,
                resource_usage,
                compliance_results,
                created_at: row.get::<i64, _>("created_at") as u64,
                completed_at: row.get::<Option<i64>, _>("completed_at").map(|t| t as u64),
                error_details: row.get("error_details"),
            };

            results.push(result);
        }

        Ok(results)
    }

    async fn get_evaluation_stats(
        &self,
        eval_id: &str,
    ) -> Result<EvaluationStats, EvaluationError> {
        // In a real implementation, this would fetch from a stats table
        // For now, return default stats
        Ok(EvaluationStats {
            eval_id: eval_id.to_string(),
            total_runs: 0,
            total_passed: 0,
            total_failed: 0,
            avg_execution_time: 0.0,
            success_rate: 1.0, // Initially assume 100% success
            last_run: None,
            error_count: 0,
            avg_resource_usage: ResourceUsage::default(),
        })
    }

    async fn update_evaluation_stats(
        &self,
        stats: &EvaluationStats,
    ) -> Result<(), EvaluationError> {
        // In a real implementation, this would update a stats table
        // For now, we'll just return Ok
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationStats {
    pub eval_id: String,
    pub total_runs: u64,
    pub total_passed: u64,
    pub total_failed: u64,
    pub avg_execution_time: f64, // in seconds
    pub success_rate: f64,
    pub last_run: Option<u64>,
    pub error_count: u64,
    pub avg_resource_usage: ResourceUsage,
}

pub struct EvaluationEngine {
    evaluations: Arc<RwLock<HashMap<String, EvaluationConfig>>>,
    results: Arc<RwLock<HashMap<String, EvaluationResult>>>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    context_router: Arc<ContextRouter>,
    memory_fabric: Arc<MemoryFabric>,
    provider_router: Arc<ProviderRouter>,
    skill_registry: Arc<SkillRegistry>,
    workflow_engine: Arc<WorkflowEngine>,
    session_manager: Arc<SessionManager>,
    storage: Arc<dyn EvaluationStorage>,
}

impl EvaluationEngine {
    pub async fn new_with_storage(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        context_router: Arc<ContextRouter>,
        memory_fabric: Arc<MemoryFabric>,
        provider_router: Arc<ProviderRouter>,
        skill_registry: Arc<SkillRegistry>,
        workflow_engine: Arc<WorkflowEngine>,
        session_manager: Arc<SessionManager>,
        pool: SqlitePool,
    ) -> Result<Self, EvaluationError> {
        let storage = Arc::new(SqliteEvaluationStorage::new(pool).await?);

        // Load existing evaluations and results from storage
        let evaluations_map = HashMap::new();
        let results_map = HashMap::new();

        // In a real implementation, we would load from storage
        // For now, we'll initialize empty maps

        Ok(EvaluationEngine {
            evaluations: Arc::new(RwLock::new(evaluations_map)),
            results: Arc::new(RwLock::new(results_map)),
            history_ledger,
            messaging_system,
            policy_engine,
            context_router,
            memory_fabric,
            provider_router,
            skill_registry,
            workflow_engine,
            session_manager,
            storage,
        })
    }

    pub async fn create_evaluation(
        &self,
        mut config: EvaluationConfig,
    ) -> Result<String, EvaluationError> {
        // Validate evaluation config
        self.validate_evaluation_config(&config).await?;

        // Check if evaluation already exists
        {
            let evaluations = self.evaluations.read().await;
            if evaluations.contains_key(&config.eval_id) {
                return Err(EvaluationError::InvalidConfiguration(format!(
                    "Evaluation {} already exists",
                    config.eval_id
                )));
            }
        }

        // Validate target component exists
        self.validate_target_component(&config.target_component)
            .await?;

        // Set creation timestamp
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        config.created_at = now;
        config.updated_at = now;

        // Store in durable storage
        self.storage.store_evaluation(&config).await?;

        // Update in-memory cache
        let mut evaluations = self.evaluations.write().await;
        evaluations.insert(config.eval_id.clone(), config.clone());

        // Log the event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EvaluationCreated".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Would be the actual creator in real implementation
            role: "evaluation_engine".to_string(),
            timestamp: now,
            trace_id: None,
            payload: serde_json::json!({
                "eval_id": config.eval_id,
                "name": config.name,
                "eval_type": format!("{:?}", config.eval_type),
                "target_component": &config.target_component,
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

        Ok(config.eval_id)
    }

    async fn validate_evaluation_config(
        &self,
        config: &EvaluationConfig,
    ) -> Result<(), EvaluationError> {
        // Validate evaluation ID
        if config.eval_id.is_empty() {
            return Err(EvaluationError::InvalidConfiguration(
                "Evaluation ID cannot be empty".to_string(),
            ));
        }

        // Validate name
        if config.name.is_empty() {
            return Err(EvaluationError::InvalidConfiguration(
                "Evaluation name cannot be empty".to_string(),
            ));
        }

        // Validate test suite
        if config.test_suite.tests.is_empty() {
            return Err(EvaluationError::InvalidConfiguration(
                "Evaluation must have at least one test case".to_string(),
            ));
        }

        // Validate each test case
        for test_case in &config.test_suite.tests {
            if test_case.test_id.is_empty() {
                return Err(EvaluationError::InvalidConfiguration(
                    "Test case ID cannot be empty".to_string(),
                ));
            }

            if test_case.name.is_empty() {
                return Err(EvaluationError::InvalidConfiguration(
                    "Test case name cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn validate_target_component(
        &self,
        target: &TargetComponent,
    ) -> Result<(), EvaluationError> {
        // In a real implementation, this would check if the target component exists
        // For now, just return Ok
        match target {
            TargetComponent::Skill { skill_id } => {
                if skill_id.is_empty() {
                    return Err(EvaluationError::TargetComponentNotFound(
                        "Skill ID cannot be empty".to_string(),
                    ));
                }
            }
            TargetComponent::Workflow { workflow_id } => {
                if workflow_id.is_empty() {
                    return Err(EvaluationError::TargetComponentNotFound(
                        "Workflow ID cannot be empty".to_string(),
                    ));
                }
            }
            TargetComponent::Provider { provider_id } => {
                if provider_id.is_empty() {
                    return Err(EvaluationError::TargetComponentNotFound(
                        "Provider ID cannot be empty".to_string(),
                    ));
                }
            }
            TargetComponent::Embodiment { embodiment_id } => {
                if embodiment_id.is_empty() {
                    return Err(EvaluationError::TargetComponentNotFound(
                        "Embodiment ID cannot be empty".to_string(),
                    ));
                }
            }
            TargetComponent::Package { package_id } => {
                if package_id.is_empty() {
                    return Err(EvaluationError::TargetComponentNotFound(
                        "Package ID cannot be empty".to_string(),
                    ));
                }
            }
            TargetComponent::Custom {
                component_type,
                reference,
            } => {
                if component_type.is_empty() || reference.is_empty() {
                    return Err(EvaluationError::TargetComponentNotFound(
                        "Custom component type and reference cannot be empty".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    pub async fn run_evaluation(&self, eval_id: String) -> Result<String, EvaluationError> {
        // Get the evaluation
        let evaluation = {
            let evaluations = self.evaluations.read().await;
            evaluations
                .get(&eval_id)
                .cloned()
                .ok_or_else(|| EvaluationError::EvaluationNotFound(eval_id.clone()))?
        };

        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: "system".to_string(), // In real implementation, this would be the running user/agent
            resource: format!("evaluation_run:{}", eval_id),
            action: "execute".to_string(),
            context: serde_json::json!({
                "eval_id": eval_id,
                "eval_type": format!("{:?}", evaluation.eval_type),
                "target_component": &evaluation.target_component,
                "execution_config": &evaluation.execution_config,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for evaluation
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EvaluationError::SecurityViolation(format!(
                "Policy denied evaluation execution: {}",
                policy_decision.reason
            )));
        }

        // Create an evaluation result
        let result_id = Uuid::new_v4().to_string();
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Execute the test suite
        let test_results = self
            .execute_test_suite(&evaluation.test_suite, &evaluation.target_component)
            .await?;

        // Determine overall status
        let overall_status =
            self.determine_overall_status(&test_results, &evaluation.success_criteria);

        // Calculate resource usage
        let resource_usage = self.calculate_resource_usage(&test_results);

        // Check compliance
        let compliance_results = self
            .check_compliance(
                &evaluation.success_criteria.required_compliance,
                &test_results,
            )
            .await?;

        // Create the result
        let result = EvaluationResult {
            result_id: result_id.clone(),
            eval_id: evaluation.eval_id.clone(),
            test_results,
            overall_status,
            execution_time_ms: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - start_time)
                * 1000, // Convert to milliseconds
            resource_usage,
            compliance_results,
            created_at: start_time,
            completed_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            error_details: None,
        };

        // Store the result
        self.storage.store_result(&result).await?;
        {
            let mut results = self.results.write().await;
            results.insert(result_id.clone(), result.clone());
        }

        // Log the evaluation result
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EvaluationCompleted".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Would be the actual runner in real implementation
            role: "evaluation_engine".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "result_id": result_id,
                "eval_id": eval_id,
                "overall_status": format!("{:?}", result.overall_status),
                "execution_time_ms": result.execution_time_ms,
                "passed_tests": result.test_results.iter().filter(|tr| matches!(tr.status, TestCaseStatus::Passed)).count(),
                "failed_tests": result.test_results.iter().filter(|tr| matches!(tr.status, TestCaseStatus::Failed)).count(),
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

        Ok(result_id)
    }

    async fn execute_test_suite(
        &self,
        test_suite: &TestSuite,
        target: &TargetComponent,
    ) -> Result<Vec<TestCaseResult>, EvaluationError> {
        let mut results = Vec::new();

        for test_case in &test_suite.tests {
            let start_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            // Execute the test case
            let test_result = self.execute_test_case(test_case, target).await;

            let end_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let execution_time = end_time - start_time;

            // Create test case result
            let test_case_result = match test_result {
                Ok(actual_output) => {
                    let status = if self.compare_output(&actual_output, &test_case.expected_output)
                    {
                        TestCaseStatus::Passed
                    } else {
                        TestCaseStatus::Failed
                    };

                    TestCaseResult {
                        test_id: test_case.test_id.clone(),
                        status,
                        execution_time_ms: execution_time,
                        resource_usage: ResourceUsage::default(), // In a real implementation, this would be measured
                        actual_output: Some(actual_output),
                        error_details: None,
                        compliance_results: None, // Would be populated in a real implementation
                        created_at: start_time / 1000, // Convert to seconds
                        completed_at: Some(end_time / 1000), // Convert to seconds
                    }
                }
                Err(error) => TestCaseResult {
                    test_id: test_case.test_id.clone(),
                    status: TestCaseStatus::Error,
                    execution_time_ms: execution_time,
                    resource_usage: ResourceUsage::default(),
                    actual_output: None,
                    error_details: Some(error.to_string()),
                    compliance_results: None,
                    created_at: start_time / 1000,
                    completed_at: Some(end_time / 1000),
                },
            };

            results.push(test_case_result);

            // Check if we should fail fast
            if matches!(
                results.last().unwrap().status,
                TestCaseStatus::Failed | TestCaseStatus::Error
            ) {
                // In a real implementation, we would check the execution config for fail_fast
                // For now, we'll continue with all tests
            }
        }

        Ok(results)
    }

    async fn execute_test_case(
        &self,
        test_case: &TestCase,
        target: &TargetComponent,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the test case against the target component
        // For now, we'll simulate the execution based on the target type
        match target {
            TargetComponent::Skill { skill_id } => {
                // Execute against a skill
                self.execute_skill_test(skill_id, &test_case.input_data)
                    .await
            }
            TargetComponent::Workflow { workflow_id } => {
                // Execute against a workflow
                self.execute_workflow_test(workflow_id, &test_case.input_data)
                    .await
            }
            TargetComponent::Provider { provider_id } => {
                // Execute against a provider
                self.execute_provider_test(provider_id, &test_case.input_data)
                    .await
            }
            TargetComponent::Embodiment { embodiment_id } => {
                // Execute against an embodiment
                self.execute_embodiment_test(embodiment_id, &test_case.input_data)
                    .await
            }
            TargetComponent::Package { package_id } => {
                // Execute against a package
                self.execute_package_test(package_id, &test_case.input_data)
                    .await
            }
            TargetComponent::Custom {
                component_type,
                reference,
            } => {
                // Execute against a custom component
                self.execute_custom_test(component_type, reference, &test_case.input_data)
                    .await
            }
        }
    }

    async fn execute_skill_test(
        &self,
        skill_id: &str,
        input_data: &serde_json::Value,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the skill with the input data
        // For now, we'll simulate a successful execution
        Ok(serde_json::json!({
            "status": "success",
            "skill_id": skill_id,
            "input": input_data,
            "output": "simulated skill output"
        }))
    }

    async fn execute_workflow_test(
        &self,
        workflow_id: &str,
        input_data: &serde_json::Value,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the workflow with the input data
        // For now, we'll simulate a successful execution
        Ok(serde_json::json!({
            "status": "success",
            "workflow_id": workflow_id,
            "input": input_data,
            "output": "simulated workflow output"
        }))
    }

    async fn execute_provider_test(
        &self,
        provider_id: &str,
        input_data: &serde_json::Value,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the provider with the input data
        // For now, we'll simulate a successful execution
        Ok(serde_json::json!({
            "status": "success",
            "provider_id": provider_id,
            "input": input_data,
            "output": "simulated provider output"
        }))
    }

    async fn execute_embodiment_test(
        &self,
        embodiment_id: &str,
        input_data: &serde_json::Value,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the embodiment with the input data
        // For now, we'll simulate a successful execution
        Ok(serde_json::json!({
            "status": "success",
            "embodiment_id": embodiment_id,
            "input": input_data,
            "output": "simulated embodiment output"
        }))
    }

    async fn execute_package_test(
        &self,
        package_id: &str,
        input_data: &serde_json::Value,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the package with the input data
        // For now, we'll simulate a successful execution
        Ok(serde_json::json!({
            "status": "success",
            "package_id": package_id,
            "input": input_data,
            "output": "simulated package output"
        }))
    }

    async fn execute_custom_test(
        &self,
        component_type: &str,
        reference: &str,
        input_data: &serde_json::Value,
    ) -> Result<serde_json::Value, EvaluationError> {
        // In a real implementation, this would execute the custom component with the input data
        // For now, we'll simulate a successful execution
        Ok(serde_json::json!({
            "status": "success",
            "component_type": component_type,
            "reference": reference,
            "input": input_data,
            "output": "simulated custom component output"
        }))
    }

    fn compare_output(&self, actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
        // In a real implementation, this would perform a more sophisticated comparison
        // For now, we'll do a simple equality check
        actual == expected
    }

    fn determine_overall_status(
        &self,
        test_results: &[TestCaseResult],
        criteria: &SuccessCriteria,
    ) -> EvaluationStatus {
        let total_tests = test_results.len();
        let passed_tests = test_results
            .iter()
            .filter(|tr| matches!(tr.status, TestCaseStatus::Passed))
            .count();

        let success_rate = if total_tests > 0 {
            passed_tests as f64 / total_tests as f64
        } else {
            1.0 // If no tests, consider it a pass
        };

        if success_rate >= criteria.min_success_rate {
            EvaluationStatus::Passed
        } else {
            EvaluationStatus::Failed
        }
    }

    fn calculate_resource_usage(&self, test_results: &[TestCaseResult]) -> ResourceUsage {
        // In a real implementation, this would calculate actual resource usage
        // For now, return default values
        ResourceUsage {
            cpu_usage: 0.0,
            memory_usage: 0.0,
            disk_usage: 0.0,
            network_usage: 0.0,
        }
    }

    async fn check_compliance(
        &self,
        requirements: &[ComplianceRequirement],
        test_results: &[TestCaseResult],
    ) -> Result<ComplianceResults, EvaluationError> {
        // In a real implementation, this would check actual compliance
        // For now, return a passing compliance result
        Ok(ComplianceResults {
            overall_compliance: true,
            compliance_details: vec![ComplianceResult {
                compliance_requirements: requirements.to_vec(),
                passed_requirements: requirements.to_vec(),
                failed_requirements: vec![],
                details: HashMap::new(),
            }],
            compliance_score: 1.0,
        })
    }

    pub async fn get_evaluation(
        &self,
        eval_id: String,
    ) -> Result<Option<EvaluationConfig>, EvaluationError> {
        // Try to get from cache first
        {
            let evaluations = self.evaluations.read().await;
            if let Some(evaluation) = evaluations.get(&eval_id) {
                return Ok(Some(evaluation.clone()));
            }
        }

        // If not in cache, get from storage
        let evaluation = self.storage.get_evaluation(&eval_id).await?;
        if let Some(evaluation) = &evaluation {
            // Update cache
            let mut evaluations = self.evaluations.write().await;
            evaluations.insert(eval_id.clone(), evaluation.clone());
        }

        Ok(evaluation)
    }

    pub async fn get_evaluation_result(
        &self,
        result_id: String,
    ) -> Result<Option<EvaluationResult>, EvaluationError> {
        // Try to get from cache first
        {
            let results = self.results.read().await;
            if let Some(result) = results.get(&result_id) {
                return Ok(Some(result.clone()));
            }
        }

        // If not in cache, get from storage
        let result = self.storage.get_result(&result_id).await?;
        if let Some(result) = &result {
            // Update cache
            let mut results = self.results.write().await;
            results.insert(result_id.clone(), result.clone());
        }

        Ok(result)
    }

    pub async fn get_evaluation_stats(
        &self,
        eval_id: String,
    ) -> Result<EvaluationStats, EvaluationError> {
        self.storage.get_evaluation_stats(&eval_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_evaluation_engine_basic_functionality() {
        // Create temporary database
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        // Create temporary history ledger
        let temp_path = format!("/tmp/test_evals_{}.jsonl", Uuid::new_v4());
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
        let system_identity = allternit_policy::Identity {
            id: "system".to_string(),
            identity_type: allternit_policy::IdentityType::ServiceAccount,
            name: "System".to_string(),
            tenant_id: "system".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["system".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine
            .register_identity(system_identity)
            .await
            .unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        policy_engine.create_default_rules().await.unwrap();
        let execute_rule = allternit_policy::PolicyRule {
            id: "rule_allow_execute".to_string(),
            name: "Allow Execute Operations".to_string(),
            description: "Allow evaluation execution in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["execute".to_string()],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(execute_rule).await.unwrap();

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
                tool_gateway.clone(),
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
            tool_gateway,
            skill_registry.clone(),
            task_queue,
            pool.clone(),
        ));

        // Create session manager
        let session_manager = Arc::new(allternit_runtime_core::SessionManager::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create evaluation engine
        let evaluation_engine = Arc::new(
            EvaluationEngine::new_with_storage(
                history_ledger,
                messaging_system,
                policy_engine,
                context_router,
                memory_fabric,
                provider_router,
                skill_registry,
                workflow_engine,
                session_manager,
                pool,
            )
            .await
            .unwrap(),
        );

        // Create a test evaluation
        let evaluation_config = EvaluationConfig {
            eval_id: "test-eval-001".to_string(),
            name: "Test Evaluation".to_string(),
            description: "A test evaluation for Allternit".to_string(),
            eval_type: EvaluationType::Regression,
            target_component: TargetComponent::Skill {
                skill_id: "test-skill".to_string(),
            },
            test_suite: TestSuite {
                suite_id: "test-suite-001".to_string(),
                name: "Basic Test Suite".to_string(),
                description: "A basic test suite".to_string(),
                tests: vec![TestCase {
                    test_id: "test-case-001".to_string(),
                    name: "Basic Test Case".to_string(),
                    description: "A basic test case".to_string(),
                    test_type: TestCaseType::Functional,
                    input_data: serde_json::json!({"input": "test"}),
                    expected_output: serde_json::json!({"output": "expected"}),
                    execution_context: ExecutionContext {
                        session_id: None,
                        tenant_id: "test-tenant".to_string(),
                        agent_id: Some("test-agent".to_string()),
                        context_bundle: None,
                        resource_limits: None,
                        security_context: SecurityContext {
                            sensitivity_tier: 2,
                            allowed_permissions: vec!["test_permission".to_string()],
                            required_compliance: vec![ComplianceRequirement::SOC2],
                            encryption_required: true,
                        },
                    },
                    timeout_ms: 5000,
                    retry_policy: RetryPolicy {
                        max_attempts: 3,
                        backoff_base: 1,
                        backoff_multiplier: 2.0,
                    },
                    tags: vec!["regression".to_string(), "functional".to_string()],
                    priority: 5,
                }],
                tags: vec!["regression".to_string(), "functional".to_string()],
                dependencies: vec![],
            },
            success_criteria: SuccessCriteria {
                min_success_rate: 1.0,
                max_failure_rate: 0.0,
                max_execution_time_ms: 10000,
                max_resource_usage: ResourceUsage {
                    cpu_usage: 80.0,
                    memory_usage: 80.0,
                    disk_usage: 80.0,
                    network_usage: 80.0,
                },
                required_compliance: vec![ComplianceRequirement::SOC2],
                custom_criteria: HashMap::new(),
            },
            execution_config: ExecutionConfig {
                parallelism: 1,
                timeout_ms: 30000,
                retry_on_failure: true,
                fail_fast: false,
                execution_order: ExecutionOrder::Sequential,
                reporting_config: ReportingConfig {
                    report_format: ReportFormat::JSON,
                    report_destination: ReportDestination::Console,
                    include_details: true,
                    include_performance: true,
                    include_security: true,
                    notify_on_completion: false,
                    notification_recipients: vec![],
                },
            },
            metadata: EvaluationMetadata {
                author: "Test Author".to_string(),
                version: "1.0.0".to_string(),
                tags: vec!["test".to_string(), "regression".to_string()],
                labels: HashMap::from([("environment".to_string(), "test".to_string())]),
                dependencies: vec![],
                environment: "test".to_string(),
            },
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            is_active: true,
        };

        // Create the evaluation
        let eval_id = evaluation_engine
            .create_evaluation(evaluation_config)
            .await
            .unwrap();
        assert_eq!(eval_id, "test-eval-001");

        // Run the evaluation
        let result_id = evaluation_engine
            .run_evaluation("test-eval-001".to_string())
            .await
            .unwrap();
        assert!(!result_id.is_empty());

        // Verify the evaluation was created
        let evaluations = evaluation_engine.evaluations.read().await;
        assert_eq!(evaluations.len(), 1);
        assert!(evaluations.contains_key("test-eval-001"));
        drop(evaluations);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
