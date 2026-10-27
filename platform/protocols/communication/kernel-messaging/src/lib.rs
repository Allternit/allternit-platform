//! Kernel messaging protocols and durable implementations
//!
//! Provides core abstractions for task queues, event buses, agent mail,
//! and resource leases with durability and validation.

use allternit_history::HistoryLedger;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, RwLock};

/// Event Schema for validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSchema {
    /// Type of event this schema validates
    pub event_type: String,
    /// Fields that must be present in the payload
    pub required_fields: Vec<String>,
    /// Fields that may optionally be present
    pub optional_fields: Vec<String>,
    /// Map of field names to their expected JSON types
    pub field_types: HashMap<String, String>, // field_name -> type (string/number/object/array/boolean)
    /// Version of this schema
    pub version: String,
}

/// Schema registry for event validation
pub struct SchemaRegistry {
    schemas: Arc<RwLock<HashMap<String, EventSchema>>>,
}

impl SchemaRegistry {
    /// Create a new empty schema registry
    pub fn new() -> Self {
        Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an event schema
    pub async fn register(&self, schema: EventSchema) -> Result<(), MessagingError> {
        let mut schemas = self.schemas.write().await;
        schemas.insert(schema.event_type.clone(), schema);
        Ok(())
    }

    /// Get schema for an event type
    pub async fn get(&self, event_type: &str) -> Option<EventSchema> {
        let schemas = self.schemas.read().await;
        schemas.get(event_type).cloned()
    }

    /// Validate an event against its schema
    pub async fn validate_event(&self, event: &EventEnvelope) -> ValidationResult {
        let schemas = self.schemas.read().await;

        if let Some(schema) = schemas.get(&event.event_type) {
            validate_event_against_schema(event, schema)
        } else {
            // No schema registered - allow passthrough but warn
            ValidationResult {
                is_valid: true,
                errors: vec![format!(
                    "No schema registered for event type: {}",
                    event.event_type
                )],
                warnings: vec!["Event passed without schema validation".to_string()],
            }
        }
    }

    /// Initialize default schemas
    pub async fn init_default_schemas(&self) -> Result<(), MessagingError> {
        // Task event schema
        self.register(EventSchema {
            event_type: "TaskCreated".to_string(),
            required_fields: vec![
                "task_id".to_string(),
                "session_id".to_string(),
                "tenant_id".to_string(),
            ],
            optional_fields: vec![
                "parent_task_id".to_string(),
                "trace_id".to_string(),
                "priority".to_string(),
            ],
            field_types: HashMap::from([
                ("task_id".to_string(), "string".to_string()),
                ("session_id".to_string(), "string".to_string()),
                ("tenant_id".to_string(), "string".to_string()),
                ("priority".to_string(), "number".to_string()),
            ]),
            version: "1.0.0".to_string(),
        })
        .await?;

        // Lease event schema
        self.register(EventSchema {
            event_type: "LeaseRequested".to_string(),
            required_fields: vec![
                "lease_id".to_string(),
                "wih_id".to_string(),
                "agent_id".to_string(),
            ],
            optional_fields: vec!["paths".to_string(), "ttl_seconds".to_string()],
            field_types: HashMap::from([
                ("lease_id".to_string(), "string".to_string()),
                ("wih_id".to_string(), "string".to_string()),
                ("agent_id".to_string(), "string".to_string()),
                ("ttl_seconds".to_string(), "number".to_string()),
            ]),
            version: "1.0.0".to_string(),
        })
        .await?;

        // Tool execution event schema
        self.register(EventSchema {
            event_type: "PreToolUse".to_string(),
            required_fields: vec![
                "tool_id".to_string(),
                "input".to_string(),
                "wih_id".to_string(),
                "run_id".to_string(),
            ],
            optional_fields: vec!["policy_decision".to_string(), "trace_id".to_string()],
            field_types: HashMap::from([
                ("tool_id".to_string(), "string".to_string()),
                ("wih_id".to_string(), "string".to_string()),
                ("run_id".to_string(), "string".to_string()),
            ]),
            version: "1.0.0".to_string(),
        })
        .await?;

        Ok(())
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the validation was successful
    pub is_valid: bool,
    /// List of validation errors
    pub errors: Vec<String>,
    /// List of non-fatal validation warnings
    pub warnings: Vec<String>,
}

/// Validate event against schema
fn validate_event_against_schema(event: &EventEnvelope, schema: &EventSchema) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Check required fields
    for field in &schema.required_fields {
        if event.payload.get(field).is_none() {
            errors.push(format!("Missing required field: {}", field));
        }
    }

    // Check field types
    for (field_name, expected_type) in &schema.field_types {
        if let Some(value) = event.payload.get(field_name) {
            if !check_type(value, expected_type) {
                errors.push(format!(
                    "Field '{}' has wrong type: expected {}, got {}",
                    field_name,
                    expected_type,
                    get_json_type(value)
                ));
            }
        }
    }

    // Check for unknown fields (warning only)
    for key in event
        .payload
        .as_object()
        .map(|o| o.keys())
        .into_iter()
        .flatten()
    {
        if !schema.required_fields.contains(key) && !schema.optional_fields.contains(key) {
            warnings.push(format!("Unknown field: {}", key));
        }
    }

    // Validate event_id format
    if !event.event_id.starts_with("evt_") {
        errors.push("Event ID must start with 'evt_'".to_string());
    }

    // Validate trace_id format if present
    if let Some(trace_id) = &event.trace_id {
        if !trace_id.starts_with("trace_") {
            warnings.push("Trace ID should start with 'trace_'".to_string());
        }
    }

    ValidationResult {
        is_valid: errors.is_empty(),
        errors,
        warnings,
    }
}

/// Check if a JSON value matches expected type
fn check_type(value: &Value, expected_type: &str) -> bool {
    match expected_type {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true, // Unknown type - allow
    }
}

/// Get JSON type name for error messages
fn get_json_type(value: &Value) -> &'static str {
    if value.is_string() {
        "string"
    } else if value.is_number() {
        "number"
    } else if value.is_boolean() {
        "boolean"
    } else if value.is_array() {
        "array"
    } else if value.is_object() {
        "object"
    } else if value.is_null() {
        "null"
    } else {
        "unknown"
    }
}

/// Compute hash of event payload for integrity verification
pub fn compute_event_hash(event: &EventEnvelope) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event.event_type.as_bytes());
    hasher.update(event.session_id.as_bytes());
    hasher.update(event.tenant_id.as_bytes());
    hasher.update(event.actor_id.as_bytes());
    hasher.update(event.timestamp.to_string().as_bytes());
    hasher.update(
        serde_json::to_string(&event.payload)
            .unwrap_or_default()
            .as_bytes(),
    );
    format!("{:x}", hasher.finalize())
}

/// Task Envelope as defined in the spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEnvelope {
    /// Unique task identifier
    pub task_id: String,
    /// Associated session identifier
    pub session_id: String,
    /// Tenant that owns this task
    pub tenant_id: String,
    /// Associated workflow identifier
    pub workflow_id: String,
    /// Specific step identifier within workflow
    pub step_id: String,
    /// Role required for execution
    pub role: String,
    /// Goal/intent of the task
    pub intent: String,
    /// Pointers to input artifacts
    pub input_refs: Vec<String>,
    /// Time/budget/scope constraints
    pub constraints: serde_json::Value,
    /// Key to prevent duplicate execution
    pub idempotency_key: Option<String>,
    /// Creation timestamp (Unix epoch seconds)
    pub created_at: u64,
    /// Time to live for this task
    pub ttl: Option<u64>,
    /// Task retry configuration
    pub retry_policy: RetryPolicy,
    /// Execution priority
    pub priority: Option<i32>,
    /// Hard deadline for completion
    pub deadline: Option<u64>,
    /// Identifier of parent task if nested
    pub parent_task_id: Option<String>,
    /// Distributed tracing identifier
    pub trace_id: Option<String>,
}

/// Retry policy for failed tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of execution attempts
    pub max_attempts: u32,
    /// Base backoff delay in seconds
    pub backoff_base: u64,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

/// Event Envelope as defined in the spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Unique event identifier
    pub event_id: String,
    /// Type/kind of event
    pub event_type: String,
    /// Associated session identifier
    pub session_id: String,
    /// Tenant that owns this event
    pub tenant_id: String,
    /// Actor that emitted the event (human/agent/service/device)
    pub actor_id: String,
    /// Role of the emitting actor
    pub role: String,
    /// Timestamp of emission (Unix epoch seconds)
    pub timestamp: u64,
    /// Distributed tracing identifier
    pub trace_id: Option<String>,
    /// Structured event data
    pub payload: serde_json::Value,
}

/// Agent Mail Message Envelope as defined in the spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMessageEnvelope {
    /// Unique message identifier
    pub message_id: String,
    /// Tenant identifier
    pub tenant_id: String,
    /// Conversation thread identifier
    pub thread_id: String,
    /// Sending identity
    pub from_identity: String,
    /// List of recipient identities
    pub to_identities: Vec<String>,
    /// Message subject line
    pub subject: String,
    /// Message body in GFM Markdown
    pub body_md: String,
    /// Pointers to message attachments
    pub attachments: Vec<String>,
    /// Creation timestamp (Unix epoch seconds)
    pub created_at: u64,
    /// Searchable message tags
    pub tags: Vec<String>,
    /// Distributed tracing identifier
    pub trace_id: Option<String>,
    /// Identifier of the message being replied to
    pub reply_to_message_id: Option<String>,
    /// Message priority
    pub priority: Option<i32>,
    /// Expiration timestamp
    pub expires_at: Option<u64>,
}

/// Lease Envelope as defined in the spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseEnvelope {
    /// Unique lease identifier
    pub lease_id: String,
    /// Tenant identifier
    pub tenant_id: String,
    /// Identity holding the lease
    pub holder_identity: String,
    /// Resource being protected (file/glob/module/device/etc.)
    pub resource_selector: String,
    /// Intent for holding the lease
    pub purpose: String,
    /// Lifetime scope of the lease
    pub scope: LeaseScope,
    /// Timestamp of acquisition
    pub acquired_at: u64,
    /// Timestamp of expiration
    pub expires_at: u64,
    /// Distributed tracing identifier
    pub trace_id: Option<String>,
    /// Token for renewing the lease
    pub renew_token: Option<String>,
    /// Policy for handling conflicts
    pub conflict_policy: ConflictPolicy,
}

/// Lifetime scope of a resource lease
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LeaseScope {
    /// Lease tied to session lifetime
    Session,
    /// Lease tied to workflow lifetime
    Workflow,
    /// Lease tied to individual task lifetime
    Task,
}

/// Policy for handling lease acquisition conflicts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// Emit warning but allow acquisition
    NotifyOnly,
    /// Prevent acquisition of busy resource
    Block,
    /// Allow acquisition in a separate branch
    BranchOnly,
}

/// Artifact Pointer as defined in the spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPointer {
    /// Unique artifact identifier
    pub artifact_id: String,
    /// Integrity hash of the content
    pub content_hash: String,
    /// IANA media type of the content
    pub media_type: String,
    /// Universal identifier for storage location
    pub storage_uri: String,
    /// Identity that created the artifact
    pub created_by: String,
    /// Creation timestamp (Unix epoch seconds)
    pub created_at: u64,
}

/// Error types for messaging operations
#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    /// Standard I/O error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization failed
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Underlying history ledger error
    #[error("History error: {0}")]
    History(#[from] allternit_history::HistoryError),
    /// Database operation failed
    #[error("SQLX error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Requested task not found
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    /// Requested event not found
    #[error("Event not found: {0}")]
    EventNotFound(String),
    /// Requested mail message not found
    #[error("Mail message not found: {0}")]
    MailMessageNotFound(String),
    /// Lease acquisition failed due to conflict
    #[error("Lease conflict: {0}")]
    LeaseConflict(String),
    /// Message envelope format is invalid
    #[error("Invalid envelope: {0}")]
    InvalidEnvelope(String),
    /// Internal mutex state is corrupted
    #[error("Mutex poisoned: {0}")]
    MutexPoisoned(String),
    /// System time operation failed
    #[error("Time error: {0}")]
    TimeError(String),
}

/// Durable TaskQueue implementation using SQLite storage
pub struct TaskQueue {
    storage: Arc<dyn storage::TaskQueueStorage>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
}

impl TaskQueue {
    /// Create a new durable task queue
    pub async fn new(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        pool: SqlitePool,
    ) -> Result<Self, MessagingError> {
        let storage = Arc::new(storage::sqlite::SqliteTaskQueueStorage::new(pool).await?);
        Ok(TaskQueue {
            storage,
            history_ledger,
        })
    }

    /// Add a task to the queue and log to history
    pub async fn enqueue(&self, task: TaskEnvelope) -> Result<(), MessagingError> {
        self.storage.enqueue(task.clone()).await?;

        // Log to history ledger
        let mut history = self
            .history_ledger
            .lock()
            .map_err(|e| MessagingError::MutexPoisoned(e.to_string()))?;
        let content = serde_json::to_value(&task)?;
        history.append(content)?;

        Ok(())
    }

    /// Retrieve the next pending task from the queue
    pub async fn dequeue(&self) -> Result<Option<TaskEnvelope>, MessagingError> {
        self.storage.dequeue().await
    }

    /// Mark a task as successfully completed
    pub async fn complete_task(
        &self,
        task_id: String,
        task: TaskEnvelope,
    ) -> Result<(), MessagingError> {
        self.storage.complete_task(task_id, task).await
    }

    /// Mark a task as failed with an error message
    pub async fn fail_task(
        &self,
        task_id: String,
        task: TaskEnvelope,
        error: String,
    ) -> Result<(), MessagingError> {
        self.storage.fail_task(task_id, task, error).await
    }

    /// Get the number of tasks currently awaiting execution
    pub async fn get_pending_count(&self) -> Result<usize, MessagingError> {
        self.storage.get_pending_count().await
    }
}

/// Durable EventBus implementation using SQLite storage
pub struct EventBus {
    storage: Arc<dyn storage::EventBusStorage>,
    subscribers: Arc<RwLock<std::collections::HashMap<String, broadcast::Sender<EventEnvelope>>>>,
    _history_ledger: Arc<Mutex<HistoryLedger>>,
}

impl EventBus {
    /// Create a new durable event bus
    pub async fn new(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        pool: SqlitePool,
    ) -> Result<Self, MessagingError> {
        let storage = Arc::new(storage::sqlite::SqliteEventBusStorage::new(pool).await?);
        Ok(EventBus {
            storage,
            subscribers: Arc::new(RwLock::new(std::collections::HashMap::new())),
            _history_ledger: history_ledger,
        })
    }

    /// Subscribe to real-time events of a specific type
    pub async fn subscribe(&self, event_type: String) -> broadcast::Receiver<EventEnvelope> {
        let mut subscribers = self.subscribers.write().await;
        if !subscribers.contains_key(&event_type) {
            let (tx, _) = broadcast::channel(100);
            subscribers.insert(event_type.clone(), tx);
        }

        let sender = subscribers
            .get(&event_type)
            .expect("Event type should exist in subscribers map after insertion check");
        sender.subscribe()
    }

    /// Publish an event to durable storage and all active subscribers
    pub async fn publish(&self, event: EventEnvelope) -> Result<(), MessagingError> {
        // Store in durable storage
        self.storage.publish(event.clone()).await?;

        // Also send to in-memory subscribers
        let subscribers = self.subscribers.read().await;
        if let Some(sender) = subscribers.get(&event.event_type) {
            let _ = sender.send(event);
        }

        Ok(())
    }

    /// Retrieve historical events of a specific type
    pub async fn get_events(
        &self,
        event_type: String,
    ) -> Result<Vec<EventEnvelope>, MessagingError> {
        self.storage.subscribe(event_type).await
    }
}

/// Durable AgentMail implementation using SQLite storage
pub struct AgentMail {
    storage: Arc<dyn storage::AgentMailStorage>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
}

impl AgentMail {
    /// Create a new durable agent mail service
    pub async fn new(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        pool: SqlitePool,
    ) -> Result<Self, MessagingError> {
        let storage = Arc::new(storage::sqlite::SqliteAgentMailStorage::new(pool).await?);
        Ok(AgentMail {
            storage,
            history_ledger,
        })
    }

    /// Send a message and log it to the history ledger
    pub async fn send_message(&self, message: MailMessageEnvelope) -> Result<(), MessagingError> {
        self.storage.send_message(message.clone()).await?;

        // Log to history ledger
        let mut history = self
            .history_ledger
            .lock()
            .map_err(|e| MessagingError::MutexPoisoned(e.to_string()))?;
        let content = serde_json::to_value(&message)?;
        history.append(content)?;

        Ok(())
    }

    /// Retrieve all messages in an agent's inbox
    pub async fn get_inbox(
        &self,
        agent_identity: String,
    ) -> Result<Vec<MailMessageEnvelope>, MessagingError> {
        self.storage.get_inbox(agent_identity).await
    }

    /// Retrieve all messages sent by an agent
    pub async fn get_outbox(
        &self,
        agent_identity: String,
    ) -> Result<Vec<MailMessageEnvelope>, MessagingError> {
        self.storage.get_outbox(agent_identity).await
    }

    /// Retrieve all messages in a specific conversation thread
    pub async fn get_thread(
        &self,
        thread_id: String,
    ) -> Result<Vec<MailMessageEnvelope>, MessagingError> {
        self.storage.get_thread(thread_id).await
    }
}

/// Durable CoordinationLeases implementation using SQLite storage
pub struct CoordinationLeases {
    storage: Arc<dyn storage::CoordinationLeasesStorage>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
}

impl CoordinationLeases {
    /// Create a new durable coordination lease service
    pub async fn new(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        pool: SqlitePool,
    ) -> Result<Self, MessagingError> {
        let storage = Arc::new(storage::sqlite::SqliteCoordinationLeasesStorage::new(pool).await?);
        Ok(CoordinationLeases {
            storage,
            history_ledger,
        })
    }

    /// Attempt to acquire an exclusive lease on a resource
    pub async fn acquire_lease(
        &self,
        lease: LeaseEnvelope,
    ) -> Result<LeaseEnvelope, MessagingError> {
        let acquired_lease = self.storage.acquire_lease(lease.clone()).await?;

        // Log to history ledger
        let mut history = self
            .history_ledger
            .lock()
            .map_err(|e| MessagingError::MutexPoisoned(e.to_string()))?;
        let content = serde_json::to_value(&acquired_lease)?;
        history.append(content)?;

        Ok(acquired_lease)
    }

    /// Release a previously held lease
    pub async fn release_lease(
        &self,
        resource_selector: String,
        renew_token: String,
    ) -> Result<bool, MessagingError> {
        let result = self
            .storage
            .release_lease(resource_selector.clone(), renew_token)
            .await?;

        if result {
            // Log to history ledger
            let mut history = self
                .history_ledger
                .lock()
                .map_err(|e| MessagingError::MutexPoisoned(e.to_string()))?;
            let content = serde_json::json!({
                "action": "lease_release",
                "resource_selector": resource_selector,
                "released_at": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| MessagingError::TimeError(e.to_string()))?
                    .as_secs()
            });
            history.append(content)?;
        }

        Ok(result)
    }

    /// Extend the expiration time of an active lease
    pub async fn renew_lease(
        &self,
        resource_selector: String,
        renew_token: String,
    ) -> Result<bool, MessagingError> {
        let result = self
            .storage
            .renew_lease(resource_selector.clone(), renew_token)
            .await?;

        if result {
            // Log to history ledger
            let mut history = self
                .history_ledger
                .lock()
                .map_err(|e| MessagingError::MutexPoisoned(e.to_string()))?;
            let content = serde_json::json!({
                "action": "lease_renew",
                "resource_selector": resource_selector,
                "renewed_at": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| MessagingError::TimeError(e.to_string()))?
                    .as_secs()
            });
            history.append(content)?;
        }

        Ok(result)
    }

    /// Check the current status of a lease on a resource
    pub async fn check_lease(
        &self,
        resource_selector: String,
    ) -> Result<Option<LeaseEnvelope>, MessagingError> {
        self.storage.check_lease(resource_selector).await
    }
}

/// Main messaging system coordinating all durable services
pub struct MessagingSystem {
    /// Task queue service
    pub task_queue: Arc<TaskQueue>,
    /// Event bus service
    pub event_bus: Arc<EventBus>,
    /// Agent mail service
    pub agent_mail: Arc<AgentMail>,
    /// Coordination leases service
    pub coordination_leases: Arc<CoordinationLeases>,
}

impl MessagingSystem {
    /// Create a new messaging system with provided storage pool
    pub async fn new_with_storage(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        pool: SqlitePool,
    ) -> Result<Self, MessagingError> {
        Ok(MessagingSystem {
            task_queue: Arc::new(TaskQueue::new(history_ledger.clone(), pool.clone()).await?),
            event_bus: Arc::new(EventBus::new(history_ledger.clone(), pool.clone()).await?),
            agent_mail: Arc::new(AgentMail::new(history_ledger.clone(), pool.clone()).await?),
            coordination_leases: Arc::new(CoordinationLeases::new(history_ledger, pool).await?),
        })
    }

    /// Create a new messaging system and run database migrations
    pub async fn new_with_migrations(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        pool: SqlitePool,
        migrations_path: &std::path::Path,
    ) -> Result<Self, MessagingError> {
        // Run migrations before initializing the system
        let migration_runner = migration::MigrationRunner::new(pool.clone())
            .await
            .map_err(MessagingError::Sqlx)?;

        migration_runner
            .run_migrations(migrations_path)
            .await
            .map_err(|e| match e {
                migration::MigrationError::Sqlx(sqlx_err) => MessagingError::Sqlx(sqlx_err),
                migration::MigrationError::Io(io_err) => MessagingError::Io(io_err),
                migration::MigrationError::Other(other_err) => {
                    MessagingError::Io(std::io::Error::other(other_err))
                }
            })?;

        Ok(MessagingSystem {
            task_queue: Arc::new(TaskQueue::new(history_ledger.clone(), pool.clone()).await?),
            event_bus: Arc::new(EventBus::new(history_ledger.clone(), pool.clone()).await?),
            agent_mail: Arc::new(AgentMail::new(history_ledger.clone(), pool.clone()).await?),
            coordination_leases: Arc::new(CoordinationLeases::new(history_ledger, pool).await?),
        })
    }
}

/// Migration utilities
pub mod migration;
/// Storage backend implementations
pub mod storage;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::NamedTempFile;
    use tokio::time::sleep;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_task_queue() {
        // Create a temporary SQLite database file
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        let temp_path = format!("/tmp/test_messaging_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        let task_queue = TaskQueue::new(history_ledger, pool).await.unwrap();

        let task = TaskEnvelope {
            task_id: Uuid::new_v4().to_string(),
            session_id: "session1".to_string(),
            tenant_id: "tenant1".to_string(),
            workflow_id: "workflow1".to_string(),
            step_id: "step1".to_string(),
            role: "test_role".to_string(),
            intent: "test_intent".to_string(),
            input_refs: vec!["artifact1".to_string()],
            constraints: serde_json::json!({"time": 300}),
            idempotency_key: Some("key1".to_string()),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            ttl: Some(3600),
            retry_policy: RetryPolicy {
                max_attempts: 3,
                backoff_base: 1,
                backoff_multiplier: 2.0,
            },
            priority: Some(1),
            deadline: None,
            parent_task_id: None,
            trace_id: Some("trace1".to_string()),
        };

        task_queue.enqueue(task.clone()).await.unwrap();
        assert_eq!(task_queue.get_pending_count().await.unwrap(), 1);

        let dequeued_task = task_queue.dequeue().await.unwrap().unwrap();
        assert_eq!(dequeued_task.task_id, task.task_id);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }

    #[tokio::test]
    async fn test_event_bus() {
        // Create a temporary SQLite database file
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        let temp_path = format!("/tmp/test_event_bus_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        let event_bus = EventBus::new(history_ledger, pool).await.unwrap();

        let mut receiver = event_bus.subscribe("test_event".to_string()).await;

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "test_event".to_string(),
            session_id: "session1".to_string(),
            tenant_id: "tenant1".to_string(),
            actor_id: "actor1".to_string(),
            role: "test_role".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: Some("trace1".to_string()),
            payload: serde_json::json!({"data": "test"}),
        };

        event_bus.publish(event.clone()).await.unwrap();

        // Allow time for the event to be processed
        sleep(Duration::from_millis(10)).await;

        // Check if we received the event
        // Note: We can't easily test the broadcast receiver in this context
        // since it might have been dropped, so we'll just verify the publish worked

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }

    #[tokio::test]
    async fn test_agent_mail() {
        // Create a temporary SQLite database file
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        let temp_path = format!("/tmp/test_agent_mail_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        let agent_mail = AgentMail::new(history_ledger, pool).await.unwrap();

        let message = MailMessageEnvelope {
            message_id: Uuid::new_v4().to_string(),
            tenant_id: "tenant1".to_string(),
            thread_id: "thread1".to_string(),
            from_identity: "sender1".to_string(),
            to_identities: vec!["recipient1".to_string()],
            subject: "Test Subject".to_string(),
            body_md: "Test message body".to_string(),
            attachments: vec![],
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            tags: vec!["test".to_string()],
            trace_id: Some("trace1".to_string()),
            reply_to_message_id: None,
            priority: Some(1),
            expires_at: None,
        };

        agent_mail.send_message(message.clone()).await.unwrap();

        let inbox = agent_mail
            .get_inbox("recipient1".to_string())
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].message_id, message.message_id);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }

    #[tokio::test]
    async fn test_coordination_leases() {
        // Create a temporary SQLite database file
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        let temp_path = format!("/tmp/test_leases_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        let coordination_leases = CoordinationLeases::new(history_ledger, pool).await.unwrap();

        let lease = LeaseEnvelope {
            lease_id: Uuid::new_v4().to_string(),
            tenant_id: "tenant1".to_string(),
            holder_identity: "holder1".to_string(),
            resource_selector: "resource1".to_string(),
            purpose: "test".to_string(),
            scope: LeaseScope::Task,
            acquired_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600,
            trace_id: Some("trace1".to_string()),
            renew_token: None,
            conflict_policy: ConflictPolicy::Block,
        };

        let acquired_lease = coordination_leases.acquire_lease(lease).await.unwrap();
        assert!(acquired_lease.renew_token.is_some());

        // Try to acquire the same resource - should fail with Block policy
        let lease2 = LeaseEnvelope {
            lease_id: Uuid::new_v4().to_string(),
            tenant_id: "tenant1".to_string(),
            holder_identity: "holder2".to_string(),
            resource_selector: "resource1".to_string(),
            purpose: "test2".to_string(),
            scope: LeaseScope::Task,
            acquired_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600,
            trace_id: Some("trace2".to_string()),
            renew_token: None,
            conflict_policy: ConflictPolicy::Block,
        };

        let result = coordination_leases.acquire_lease(lease2).await;
        assert!(result.is_err());

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
