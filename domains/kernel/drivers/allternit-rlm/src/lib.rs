//! # Allternit Recursive Language Model (RLM) Framework
//!
//! Implements the RLM paradigm for long-horizon reasoning tasks following research from
//! https://www.primeintellect.ai/blog/rlm and https://arxiv.org/abs/2512.24601.
//!
//! ## Architecture Overview
//!
//! The RLM framework follows Unix philosophy with clear separation of concerns:
//! 1. A root RLM agent that only has access to a Python REPL environment
//! 2. Helper sub-models that handle tool execution and heavy lifting
//! 3. A persistent memory layer for state management
//! 4. Context management for recursive reasoning
//!
//! The root model operates by:
//! - Loading full context into a Python REPL as a string variable
//! - Never seeing the full context directly in its prompt
//! - Using code to slice, filter, and call sub-LLMs on chunks
//! - Aggregating results programmatically

use allternit_context_router::ContextRouter;
use allternit_history::HistoryLedger;
use allternit_memory::MemoryFabric;
use allternit_messaging::MessagingSystem;
use allternit_policy::PolicyEngine;
use allternit_registry::UnifiedRegistry;
use allternit_skills::SkillRegistry;
use allternit_tools_gateway::{
    run_scoped_write_scope, FilesystemAccess, NetworkAccess, ResourceLimits, ToolDefinition,
    ToolExecutionRequest, ToolExecutionResult, ToolGateway, ToolType,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod memory_policy;
pub mod modes;
pub mod router;

pub use memory_policy::{DefaultMemoryPolicy, MemoryPolicy, MemoryPolicyInput, MemoryPolicyOutput};
pub use modes::base::{ExecutionMode as RLMExecutionMode, ModeExecutor, ModeSelectionConfig};
pub use modes::hybrid::{HybridModeExecutor, StandardModeExecutor};
pub use modes::rlm::RLMModeExecutor;
pub use modes::session::{
    DiffDetail, EntryType, ExecutionEntry, RLMSession, SessionDiff, SessionManager,
    SessionMetadata, SessionState, TagInfo,
};
pub use modes::unix::{UnixAgentConfig, UnixExecutor, UnixOutput};
pub use router::RLMRouter;

#[derive(Debug, thiserror::Error)]
pub enum RLMError {
    #[error("Python execution error: {0}")]
    PythonExecution(String),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Policy error: {0}")]
    Policy(#[from] allternit_policy::PolicyError),
    #[error("Memory error: {0}")]
    Memory(#[from] allternit_memory::MemoryError),
    #[error("History error: {0}")]
    History(#[from] allternit_history::HistoryError),
    #[error("Messaging error: {0}")]
    Messaging(#[from] allternit_messaging::MessagingError),
    #[error("Registry error: {0}")]
    Registry(#[from] allternit_registry::RegistryError),
    #[error("Skills error: {0}")]
    Skills(#[from] allternit_skills::SkillsError),
    #[error("Tools gateway error: {0}")]
    Tools(#[from] allternit_tools_gateway::ToolGatewayError),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Validation failed: {0}")]
    Validation(String),
    #[error("Session error: {0}")]
    Session(#[from] crate::modes::session::SessionError),
    #[error("Internal error: {0}")]
    BoxError(String),
}

impl From<Box<dyn std::error::Error + Send + Sync>> for RLMError {
    fn from(err: Box<dyn std::error::Error + Send + Sync>) -> Self {
        RLMError::BoxError(err.to_string())
    }
}

/// Configuration for the RLM framework
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLMConfig {
    /// ID of the root model for context management
    pub root_model_id: String,

    /// ID of the sub-model for tool execution
    pub sub_model_id: String,

    /// Maximum parallel sub-LLM calls
    pub max_parallel_calls: usize,

    /// Maximum recursion depth
    pub max_recursion_depth: u32,

    /// Enable Python Gateway execution
    pub enable_python_sandbox: bool,

    /// Memory persistence settings
    pub memory_settings: RLMmemorySettings,
}

impl Default for RLMConfig {
    fn default() -> Self {
        Self {
            root_model_id: "claude-opus".to_string(),
            sub_model_id: "claude-sonnet".to_string(),
            max_parallel_calls: 10,
            max_recursion_depth: 5,
            enable_python_sandbox: true,
            memory_settings: RLMmemorySettings::default(),
        }
    }
}

/// Memory settings for RLM operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLMmemorySettings {
    /// Working memory retention period (seconds)
    pub working_memory_ttl: u64,

    /// Episodic memory retention period (seconds)
    pub episodic_memory_ttl: u64,

    /// Knowledge graph retention period (seconds)
    pub knowledge_graph_ttl: u64,
}

impl Default for RLMmemorySettings {
    fn default() -> Self {
        Self {
            working_memory_ttl: 3600,
            episodic_memory_ttl: 86400,
            knowledge_graph_ttl: 2592000,
        }
    }
}

use allternit_providers::{
    Capability, Modality, Persona, ProviderBudget, ProviderRequest, ProviderRouter,
};

/// The main RLM orchestrator that manages the root/sub-model interaction
pub struct RLMOrchestrator {
    config: RLMConfig,
    policy_engine: Arc<PolicyEngine>,
    memory_fabric: Arc<MemoryFabric>,
    unified_registry: Arc<UnifiedRegistry>,
    skill_registry: Arc<SkillRegistry>,
    tool_gateway: Arc<ToolGateway>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    provider_router: Arc<ProviderRouter>,
    python_gateway_tool_id: String,
    session_manager: Arc<SessionManager>,
    sqlite_pool: Arc<SqlitePool>,
}

impl RLMOrchestrator {
    pub async fn new(
        config: RLMConfig,
        policy_engine: Arc<PolicyEngine>,
        memory_fabric: Arc<MemoryFabric>,
        unified_registry: Arc<UnifiedRegistry>,
        skill_registry: Arc<SkillRegistry>,
        tool_gateway: Arc<ToolGateway>,
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        provider_router: Arc<ProviderRouter>,
        sqlite_pool: Arc<SqlitePool>,
    ) -> Result<Self, RLMError> {
        let python_gateway_url = std::env::var("PYTHON_GATEWAY_URL")
            .unwrap_or_else(|_| "http://localhost:8000".to_string());
        let python_gateway_tool_id = std::env::var("PYTHON_GATEWAY_TOOL_ID")
            .unwrap_or_else(|_| "python-gateway-exec".to_string());
        let endpoint = format!("{}/v1/tools/execute", python_gateway_url);

        let tool = ToolDefinition {
            id: python_gateway_tool_id.clone(),
            name: "Python Gateway Executor".to_string(),
            description: "Execute Python code via Python Gateway service".to_string(),
            tool_type: ToolType::Http,
            command: String::new(),
            endpoint,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "context_length": { "type": "number" },
                    "session_id": { "type": "string" }
                }
            }),
            output_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": { "type": ["string", "null"] }
                }
            }),
            side_effects: vec!["python_execution".to_string()],
            idempotency_behavior: "non-idempotent".to_string(),
            retryable: false,
            failure_classification: "execution".to_string(),
            safety_tier: allternit_policy::SafetyTier::T1,
            resource_limits: ResourceLimits {
                cpu: Some("1".to_string()),
                memory: Some("512MB".to_string()),
                network: NetworkAccess::DomainAllowlist(vec![python_gateway_url]),
                filesystem: FilesystemAccess::None,
                time_limit: 30,
            },
            subprocess: None,
        };

        // Register the Python gateway tool
        tool_gateway
            .register_tool(tool)
            .await
            .map_err(RLMError::Tools)?;

        let session_pool = sqlite_pool.as_ref().clone();
        Ok(Self {
            config,
            policy_engine,
            memory_fabric,
            unified_registry,
            skill_registry,
            tool_gateway,
            history_ledger,
            messaging_system,
            provider_router,
            python_gateway_tool_id,
            sqlite_pool: sqlite_pool.clone(),
            session_manager: Arc::new(
                SessionManager::new(session_pool)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?,
            ),
        })
    }

    /// Create an RLMRouter instance
    pub async fn create_router(
        &self,
        context_router: Arc<ContextRouter>,
    ) -> Result<RLMRouter, RLMError> {
        RLMRouter::new(
            self.config.clone(),
            self.policy_engine.clone(),
            self.memory_fabric.clone(),
            self.unified_registry.clone(),
            context_router,
            self.provider_router.clone(),
            self.sqlite_pool.clone(),
        )
        .await
    }

    /// Execute a task using the RLM paradigm
    /// Execute a task using the RLM paradigm with REPL control plane
    pub async fn execute_rlm_task(&self, task: RLMTask) -> Result<RLMResult, RLMError> {
        let repl_env = RLMEnvironment::new(
            task.context.clone(),
            task.session_id.clone(),
            self.memory_fabric.clone(),
            self.unified_registry.clone(),
            self.tool_gateway.clone(),
            self.python_gateway_tool_id.clone(),
        )?;

        self.session_manager
            .commit(
                &task.description,
                SessionState {
                    context: task.context.clone(),
                    recursion_depth: 0,
                    execution_log: vec![],
                    variables: HashMap::new(),
                    answer: None,
                },
                vec![],
                "RLM",
                &task.session_id,
            )
            .await?;

        let mut current_depth = 0;
        let mut final_answer = None;

        while current_depth < self.config.max_recursion_depth && final_answer.is_none() {
            let code = self.generate_code_for_state(&task, current_depth).await?;

            let execution_result = repl_env.execute_code(&code).await?;

            let entry = ExecutionEntry {
                entry_type: EntryType::Execution,
                content: format!("Code: {}", code),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                parent_hash: None,
                hash: Uuid::new_v4().to_string(),
            };

            self.session_manager
                .add_entry(&task.session_id, entry)
                .await?;

            if let Some(answer) = &execution_result.answer {
                final_answer = Some(answer.clone());
            }

            current_depth += 1;
        }

        let final_state = repl_env.get_state().await?;

        let answer = final_answer.ok_or_else(|| {
            RLMError::Validation("Max recursion depth reached without complete answer".to_string())
        })?;

        let result_data = serde_json::json!({
            "task": &task.description,
            "context_length": task.context.len(),
            "result": &answer,
            "recursion_depth": current_depth,
            "processed_at": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });

        let _result_id = self
            .store_rlm_processing_result(&result_data, &task.session_id)
            .await?;

        let result_entry = ExecutionEntry {
            entry_type: EntryType::Result,
            content: format!("Answer: {}", answer),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            parent_hash: None,
            hash: Uuid::new_v4().to_string(),
        };

        self.session_manager
            .add_entry(&task.session_id, result_entry)
            .await?;

        Ok(RLMResult {
            answer,
            execution_log: vec![],
            final_state,
        })
    }

    /// Generate code for the current state of the RLM execution via Root LLM
    async fn generate_code_for_state(
        &self,
        task: &RLMTask,
        depth: u32,
    ) -> Result<String, RLMError> {
        let system_prompt = r#"You are the Root RLM Agent. You have a Python REPL environment with a large text variable named 'context'.
Your goal is to answer the user's task by writing Python code.
You CANNOT see the 'context' variable directly. You must inspect it using code (len(context), etc).
You have a helper function: `ask_sub_model(prompt, content_chunk) -> str`.
Use this to delegate analysis of specific chunks of the context to a sub-model.

Write Python code that:
1. Analyzes the 'context' (slicing, searching).
2. Calls `ask_sub_model` on relevant chunks.
3. Aggregates results.
4. Assigns the final result to a variable named `answer`.
5. If you cannot answer yet, do not assign `answer`.

Example:
chunks = [context[i:i+1000] for i in range(0, len(context), 1000)]
summaries = [ask_sub_model("Summarize this", chunk) for chunk in chunks]
answer = "\n".join(summaries)
"#;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let context_bundle = allternit_context_router::ContextBundle {
            bundle_id: Uuid::new_v4().to_string(),
            tenant_id: task.tenant_id.clone(),
            session_id: Some(task.session_id.clone()),
            created_at: now,
            expires_at: None,
            context_entries: vec![],
            provenance: allternit_context_router::ContextProvenance {
                origin_session: Some(task.session_id.clone()),
                origin_agent: "rlm-root".to_string(),
                derivation_chain: vec![],
                integrity_hash: "placeholder".to_string(),
                signature: None,
            },
            access_control: allternit_context_router::ContextAccessControl {
                allowed_agents: std::collections::HashSet::new(),
                allowed_skills: std::collections::HashSet::new(),
                allowed_phases: std::collections::HashSet::new(),
                time_window: None,
                access_policy: allternit_context_router::ContextAccessPolicy::ExplicitAllowList,
            },
            size_bytes: 0,
            last_accessed: now,
            access_count: 0,
        };

        let request = ProviderRequest {
            request_id: Uuid::new_v4().to_string(),
            session_id: task.session_id.clone(),
            tenant_id: task.tenant_id.clone(),
            agent_id: "rlm-root".to_string(),
            persona: Persona {
                persona_id: "rlm-root".to_string(),
                name: "RLM Root".to_string(),
                description: "Recursive Language Model Root Agent".to_string(),
                base_persona: "system".to_string(),
                overlays: vec![],
                version: "1.0.0".to_string(),
                created_at: 0,
                updated_at: 0,
                is_active: true,
            },
            context_bundle, // No context loaded directly
            intent: format!(
                "Task: {}\nCurrent Recursion Depth: {}",
                task.description, depth
            ),
            required_capabilities: vec![Capability {
                model: self.config.root_model_id.clone(),
                modalities: vec![Modality::Text],
                context_window: 128000,
                max_tokens: 4096,
                response_time_ms: 0,
                cost_per_token: 0.0,
                safety_tier: 0,
            }],
            budget_constraints: ProviderBudget {
                daily_limit: None,
                monthly_limit: None,
                rate_limit: None,
                token_limit: None,
            },
            trace_id: task.trace_id.clone(),
        };

        let response = self
            .provider_router
            .route_request(request)
            .await
            .map_err(|e| RLMError::Validation(format!("Root model generation failed: {}", e)))?;

        let code = response
            .response
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| response.response.to_string());

        // Clean up markdown code blocks if present
        let code = code.replace("```python", "").replace("```", "");

        Ok(code)
    }

    /// Store RLM processing results using memory fabric capabilities
    async fn store_rlm_processing_result(
        &self,
        result_data: &serde_json::Value,
        session_id: &str,
    ) -> Result<String, RLMError> {
        self.memory_fabric
            .store_rlm_processing_result(
                result_data,
                session_id,
                "rlm-orchestrator".to_string(),
                vec![],
            )
            .await
            .map_err(RLMError::Memory)
    }
}

/// Represents a task to be executed using the RLM paradigm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLMTask {
    /// Unique task ID
    pub id: String,

    /// Description of the task
    pub description: String,

    /// Full context to be loaded into the REPL
    pub context: String,

    /// Tenant ID for isolation
    pub tenant_id: String,

    /// Session ID for tracking
    pub session_id: String,

    /// Optional trace ID for distributed tracing
    pub trace_id: Option<String>,
}

impl RLMTask {
    pub fn new(
        description: String,
        context: String,
        tenant_id: String,
        session_id: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            description,
            context,
            tenant_id,
            session_id,
            trace_id: None,
        }
    }
}

/// Represents the result of an RLM execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLMResult {
    /// The final answer produced by the RLM
    pub answer: String,

    /// Log of all code executions during the process
    pub execution_log: Vec<ExecutionStep>,

    /// Final state of the REPL environment
    pub final_state: RLMState,
}

/// Represents a single execution step in the RLM process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// The code that was executed
    pub code: String,

    /// The result of the execution
    pub result: String,

    /// Timestamp of execution
    pub timestamp: u64,
}

/// Current state of the RLM execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLMState {
    /// Current answer (None if not yet determined)
    pub answer: Option<String>,

    /// Variables in the Python namespace
    pub variables: HashMap<String, String>,

    /// Execution depth
    pub depth: u32,

    /// Call stack for tracking recursive calls
    pub call_stack: Vec<String>,
}

/// The RLM environment that provides the REPL control plane for long-context management
pub struct RLMEnvironment {
    /// The context string loaded into the environment as a variable
    context: String,

    /// Current answer variable in the REPL namespace
    answer: Option<String>,

    /// Execution log
    execution_log: Arc<RwLock<Vec<ExecutionStep>>>,

    python_gateway_tool_id: String,
    tool_gateway: Arc<ToolGateway>,

    /// Memory fabric for persistence
    memory_fabric: Arc<MemoryFabric>,

    /// Unified registry for capability access
    unified_registry: Arc<UnifiedRegistry>,

    /// Session ID for tracking
    session_id: String,
}

impl RLMEnvironment {
    pub fn new(
        context: String,
        session_id: String,
        memory_fabric: Arc<MemoryFabric>,
        unified_registry: Arc<UnifiedRegistry>,
        tool_gateway: Arc<ToolGateway>,
        python_gateway_tool_id: String,
    ) -> Result<Self, RLMError> {
        Ok(Self {
            context,
            answer: None,
            execution_log: Arc::new(RwLock::new(Vec::new())),
            python_gateway_tool_id,
            tool_gateway,
            memory_fabric,
            unified_registry,
            session_id,
        })
    }

    /// Execute code in the RLM environment via Python Gateway service
    pub async fn execute_code(&self, code: &str) -> Result<RLMExecutionResult, RLMError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let python_code_with_context = format!(
            "import json\ncontext = \"\"\"{}\"\"\"\nanswer = None\n{}\nresult = {{\n    \"answer\": answer,\n    \"context_length\": len(context),\n    \"code_executed\": True\n}}\n",
            self.context.replace("\"\"\"", "\\\"\\\"\\\""),
            code
        );

        let run_id = uuid::Uuid::new_v4().to_string();
        let execution_request = ToolExecutionRequest {
            tool_id: self.python_gateway_tool_id.clone(),
            input: serde_json::json!({
                "code": python_code_with_context,
                "context_length": self.context.len(),
                "session_id": &self.session_id,
            }),
            identity_id: "rlm-environment".to_string(),
            session_id: self.session_id.clone(),
            tenant_id: "rlm".to_string(),
            run_id: Some(run_id.clone()),
            workflow_id: Some("rlm-session".to_string()),
            node_id: Some("rlm-exec".to_string()),
            wih_id: Some("wih-rlm".to_string()),
            write_scope: Some(run_scoped_write_scope(&run_id, false)),
            capsule_run: None,
            trace_id: None,
            retry_count: 0,
            idempotency_key: None,
        };

        let result: ToolExecutionResult = self
            .tool_gateway
            .execute_tool(execution_request)
            .await
            .map_err(RLMError::Tools)?;

        let execution_step = ExecutionStep {
            code: code.to_string(),
            result: result.stdout.clone(),
            timestamp,
        };

        self.execution_log.write().await.push(execution_step);

        let new_answer = result
            .output
            .as_ref()
            .and_then(|output| output.get("answer"))
            .and_then(|answer_val| answer_val.as_str())
            .map(|answer| answer.to_string());

        Ok(RLMExecutionResult {
            success: result.exit_code.unwrap_or(1) == 0 && result.error.is_none(),
            result: if !result.stderr.is_empty() {
                format!("{}\nSTDERR: {}", result.stdout, result.stderr)
            } else if !result.stdout.is_empty() {
                result.stdout
            } else if let Some(output) = &result.output {
                output.to_string()
            } else {
                String::new()
            },
            answer: new_answer,
            execution_log: vec![],
        })
    }

    /// Get the current state of the environment
    pub async fn get_state(&self) -> Result<RLMState, RLMError> {
        let execution_log = self.execution_log.read().await.clone();

        Ok(RLMState {
            answer: self.answer.clone(),
            variables: HashMap::new(),
            depth: 0,
            call_stack: vec![],
        })
    }
}

/// Unix-mode executor trait for composable, observable execution
pub trait UnixModeExecutor: Send + Sync {
    /// Execute a task in a stateless, pipe-friendly manner
    async fn execute_from_stdin(&self, task: RLMTask) -> Result<RLMResult, RLMError>;

    /// Execute with structured input/output
    async fn execute_structured(&self, task: RLMTask) -> Result<RLMResult, RLMError>;
}

impl UnixModeExecutor for RLMOrchestrator {
    async fn execute_from_stdin(&self, task: RLMTask) -> Result<RLMResult, RLMError> {
        self.execute_rlm_task(task).await
    }

    async fn execute_structured(&self, task: RLMTask) -> Result<RLMResult, RLMError> {
        self.execute_rlm_task(task).await
    }
}

/// Hybrid mode executor that automatically selects the best execution mode
pub struct HybridExecutor {
    rlm_orchestrator: Arc<RLMOrchestrator>,
    config: RLMConfig,
    mode_selection_config: ModeSelectionConfig,
}

impl HybridExecutor {
    pub fn new(orchestrator: Arc<RLMOrchestrator>, config: RLMConfig) -> Self {
        Self {
            rlm_orchestrator: orchestrator,
            config,
            mode_selection_config: ModeSelectionConfig::default(),
        }
    }

    /// Execute a task using the most appropriate mode based on context characteristics
    pub async fn execute(&self, task: RLMTask) -> Result<RLMResult, RLMError> {
        let context_size = task.context.len();
        let task_complexity = self.assess_task_complexity(&task.description);

        // Determine the best execution approach based on context size and complexity
        match task_complexity {
            crate::modes::base::TaskComplexity::Simple if context_size < 32_000 => {
                self.rlm_orchestrator.execute_rlm_task(task).await
            }
            crate::modes::base::TaskComplexity::Complex
            | crate::modes::base::TaskComplexity::LongHorizon => {
                // Use RLM for complex reasoning tasks regardless of context size
                self.rlm_orchestrator.execute_rlm_task(task).await
            }
            _ if context_size > 128_000 => {
                // Use RLM for very large contexts that may require recursive processing
                self.rlm_orchestrator.execute_rlm_task(task).await
            }
            _ => {
                // Default to RLM for other cases
                self.rlm_orchestrator.execute_rlm_task(task).await
            }
        }
    }

    /// Assess task complexity using multiple heuristics
    fn assess_task_complexity(&self, task_description: &str) -> crate::modes::base::TaskComplexity {
        let task_lower = task_description.to_lowercase();

        // Count complexity indicators
        let mut complexity_score = 0;

        for pattern in &self.mode_selection_config.complexity_patterns {
            if task_lower.contains(&pattern.replace(".*", " ")) {
                complexity_score += 2;
            }
        }

        if task_lower.contains("across") {
            complexity_score += 1;
        }
        if task_lower.contains("entire") {
            complexity_score += 1;
        }
        if task_lower.contains("all") {
            complexity_score += 1;
        }
        if task_lower.contains("comprehensive") {
            complexity_score += 2;
        }
        if task_lower.contains("multi") {
            complexity_score += 1;
        }
        if task_lower.contains("complex") {
            complexity_score += 2;
        }
        if task_lower.contains("large") {
            complexity_score += 1;
        }
        if task_lower.contains("long") {
            complexity_score += 1;
        }
        if task_lower.contains("analyze") {
            complexity_score += 1;
        }
        if task_lower.contains("compare") {
            complexity_score += 1;
        }
        if task_lower.contains("synthesize") {
            complexity_score += 2;
        }
        if task_lower.contains("reason") {
            complexity_score += 1;
        }
        if task_lower.contains("plan") {
            complexity_score += 1;
        }
        if task_lower.contains("strategy") {
            complexity_score += 2;
        }

        // Classify based on score
        if complexity_score >= 8 {
            crate::modes::base::TaskComplexity::LongHorizon
        } else if complexity_score >= 5 {
            crate::modes::base::TaskComplexity::Complex
        } else if complexity_score >= 3 {
            crate::modes::base::TaskComplexity::LargeScale
        } else {
            crate::modes::base::TaskComplexity::Simple
        }
    }
}

/// Result of executing code in the RLM environment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLMExecutionResult {
    pub success: bool,
    pub result: String,
    pub answer: Option<String>,
    pub execution_log: Vec<ExecutionStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rlm_task_creation() {
        let task = RLMTask::new(
            "Test task".to_string(),
            "Test context".to_string(),
            "tenant1".to_string(),
            "session1".to_string(),
        );

        assert_eq!(task.description, "Test task");
        assert_eq!(task.context, "Test context");
        assert_eq!(task.tenant_id, "tenant1");
        assert_eq!(task.session_id, "session1");
        assert!(!task.id.is_empty());
    }
}
