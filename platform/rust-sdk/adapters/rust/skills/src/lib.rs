use allternit_history::HistoryLedger;
use allternit_messaging::{EventEnvelope, MessagingSystem};
use allternit_policy::{PolicyEngine, SafetyTier};
use allternit_tools_gateway::{ToolDefinition, ToolGateway};
use data_encoding::{BASE64, BASE64URL, HEXLOWER};
use ring::{digest, signature};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillManifest {
    pub id: String, // global unique; reverse-DNS recommended
    pub name: String,
    pub version: String, // semver
    pub description: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,

    // Contract (Typed I/O)
    pub inputs: SkillIO,
    pub outputs: SkillIO,

    // Execution Requirements
    pub runtime: SkillRuntime,
    pub environment: SkillEnvironment,

    // Side Effects (Declared)
    pub side_effects: Vec<String>,

    // Governance
    pub risk_tier: SafetyTier,
    pub required_permissions: Vec<String>,
    pub requires_policy_gate: bool,
    pub publisher: PublisherInfo,
    pub signature: SignatureInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillIO {
    pub schema: String, // path/ref to JSON Schema
    pub examples: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillRuntime {
    pub mode: RuntimeMode,
    pub timeouts: SkillTimeouts,
    pub resources: Option<ResourceHints>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub enum RuntimeMode {
    Sandbox,
    Host,
    Container,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillTimeouts {
    pub per_step: Option<u64>, // seconds
    pub total: Option<u64>,    // seconds
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResourceHints {
    pub cpu: Option<String>,
    pub gpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillEnvironment {
    pub allowed_envs: Vec<Environment>,
    pub network: NetworkAccess,
    pub filesystem: FilesystemAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub enum Environment {
    Dev,
    Stage,
    Prod,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub enum NetworkAccess {
    None,
    DomainAllowlist(Vec<String>),
    Unrestricted,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub enum FilesystemAccess {
    None,
    Allowlist(Vec<String>),
    ReadWrite(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PublisherInfo {
    pub publisher_id: String,
    pub public_key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SignatureInfo {
    pub manifest_sig: String,
    pub bundle_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillWorkflow {
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub per_node_constraints: HashMap<String, NodeConstraints>,
    pub artifact_outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkflowNode {
    pub id: String,
    pub name: String,
    pub phase: WorkflowPhase,
    pub tool_binding: String, // references a tool in the skill's tools/
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, utoipa::ToSchema)]
pub enum WorkflowPhase {
    Observe,
    Think,
    Plan,
    Build,
    Execute,
    Verify,
    Learn,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    pub condition: Option<String>, // optional condition expression
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeConstraints {
    pub time_budget: Option<u64>, // seconds
    pub resource_limits: Option<ResourceHints>,
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Skill {
    pub manifest: SkillManifest,
    pub workflow: SkillWorkflow,
    pub tools: Vec<ToolDefinition>,
    pub human_routing: String, // content of SKILL.md
}

#[derive(Debug, thiserror::Error)]
pub enum SkillsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] allternit_history::HistoryError),
    #[error("SQLX error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Skill not found: {0}")]
    SkillNotFound(String),
    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),
    #[error("Signature verification failed: {0}")]
    SignatureVerification(String),
}

// Publisher key management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherKey {
    pub publisher_id: String,
    pub public_key_id: String,
    pub public_key: String,
    pub created_at: u64,
    pub revoked: bool,
    pub revoked_at: Option<u64>,
}

fn canonicalize_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let mut normalized = serde_json::Map::new();
            for key in keys {
                if let Some(value) = map.get(key) {
                    normalized.insert(key.clone(), canonicalize_json_value(value));
                }
            }
            serde_json::Value::Object(normalized)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonicalize_json_value).collect())
        }
        _ => value.clone(),
    }
}

fn build_skill_bundle_payload(skill: &Skill) -> serde_json::Value {
    let mut manifest = skill.manifest.clone();
    manifest.signature.manifest_sig = String::new();
    manifest.signature.bundle_hash = String::new();
    serde_json::json!({
        "manifest": manifest,
        "workflow": skill.workflow.clone(),
        "tools": skill.tools.clone(),
        "human_routing": skill.human_routing.clone(),
    })
}

pub fn canonical_skill_payload(skill: &Skill) -> Result<Vec<u8>, SkillsError> {
    let payload = build_skill_bundle_payload(skill);
    let canonical = canonicalize_json_value(&payload);
    serde_json::to_vec(&canonical).map_err(SkillsError::Json)
}

pub fn compute_bundle_digest(skill: &Skill) -> Result<[u8; 32], SkillsError> {
    let payload = canonical_skill_payload(skill)?;
    let digest = digest::digest(&digest::SHA256, &payload);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(digest.as_ref());
    Ok(bytes)
}

pub fn compute_bundle_hash(skill: &Skill) -> Result<String, SkillsError> {
    let digest = compute_bundle_digest(skill)?;
    Ok(format!("sha256:{}", HEXLOWER.encode(&digest)))
}

fn normalize_bundle_hash(input: &str) -> Result<String, SkillsError> {
    let trimmed = input.trim();
    let without_prefix = trimmed.strip_prefix("sha256:").unwrap_or(trimmed);
    let normalized = without_prefix.to_lowercase();
    if normalized.len() != 64 || !normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(SkillsError::Validation(
            "bundle_hash must be a sha256 hex digest".to_string(),
        ));
    }
    Ok(normalized)
}

fn decode_base64(input: &str) -> Result<Vec<u8>, SkillsError> {
    BASE64
        .decode(input.as_bytes())
        .or_else(|_| BASE64URL.decode(input.as_bytes()))
        .map_err(|_| SkillsError::Validation("Invalid base64 encoding".to_string()))
}

// Channel types for skill distribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Channel {
    Stable,
    Beta,
    Dev,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Stable => write!(f, "stable"),
            Channel::Beta => write!(f, "beta"),
            Channel::Dev => write!(f, "dev"),
        }
    }
}

// Storage trait for skill registry
pub mod storage;
pub mod browser_use;
pub mod ui_tars;

/// Skill Creator - Primitive for creating effective skills
///
/// Provides foundational capabilities for skill creation including:
/// - Session-based skill development workflow
/// - Templates for common skill types
/// - Validation of skill structure and content
/// - Packaging into distributable `.skill` files
pub mod creator;

pub struct SkillRegistry {
    storage: Arc<dyn storage::SkillStorage>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    tool_gateway: Arc<ToolGateway>,
}

impl SkillRegistry {
    pub async fn new_with_storage(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        tool_gateway: Arc<ToolGateway>,
        pool: SqlitePool,
    ) -> Result<Self, SkillsError> {
        let storage = Arc::new(storage::SqliteSkillStorage::new(pool).await?);

        Ok(SkillRegistry {
            storage,
            history_ledger,
            messaging_system,
            policy_engine,
            tool_gateway,
        })
    }

    pub async fn register_skill(&self, skill: Skill) -> Result<String, SkillsError> {
        // Validate the skill manifest
        self.validate_skill(&skill).await?;

        // Verify the signature
        self.verify_signature(&skill).await?;

        // Store the skill in durable storage
        self.storage.store_skill(&skill).await?;

        // Register skill's tools with the tool gateway
        for tool in &skill.tools {
            if let Err(e) = self.tool_gateway.register_tool(tool.clone()).await {
                tracing::warn!(
                    "Failed to register tool {} from skill {}: {}",
                    tool.id,
                    skill.manifest.id,
                    e
                );
            } else {
                tracing::info!(
                    "Registered tool {} from skill {}",
                    tool.id,
                    skill.manifest.id
                );
            }
        }

        // Log to history ledger
        let mut history = self.history_ledger.lock().unwrap();
        let content = serde_json::to_value(&skill)?;
        history.append(content)?;

        // Emit skill install event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "SkillInstall".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(),
            role: "skill_registry".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "skill_id": skill.manifest.id,
                "skill_name": skill.manifest.name,
                "version": skill.manifest.version,
                "publisher_id": skill.manifest.publisher.publisher_id,
                "tools_registered": skill.tools.len()
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(skill.manifest.id.clone())
    }

    async fn validate_skill(&self, skill: &Skill) -> Result<(), SkillsError> {
        // Validate manifest fields
        if skill.manifest.id.is_empty() {
            return Err(SkillsError::Validation(
                "Skill ID cannot be empty".to_string(),
            ));
        }

        if skill.manifest.name.is_empty() {
            return Err(SkillsError::Validation(
                "Skill name cannot be empty".to_string(),
            ));
        }
        if skill.manifest.publisher.publisher_id.trim().is_empty() {
            return Err(SkillsError::Validation(
                "Publisher id cannot be empty".to_string(),
            ));
        }
        if skill.manifest.publisher.public_key_id.trim().is_empty() {
            return Err(SkillsError::Validation(
                "Publisher public_key_id cannot be empty".to_string(),
            ));
        }

        // Validate version format (simple semver check)
        let version_parts: Vec<&str> = skill.manifest.version.split('.').collect();
        if version_parts.len() != 3 {
            return Err(SkillsError::Validation(
                "Version must be in semver format (x.y.z)".to_string(),
            ));
        }

        // Validate that all tools referenced in workflow exist
        for node in &skill.workflow.nodes {
            let mut found = false;
            for tool in &skill.tools {
                if tool.id == node.tool_binding {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(SkillsError::Validation(format!(
                    "Workflow node {} references non-existent tool {}",
                    node.id, node.tool_binding
                )));
            }
        }

        // Validate that all referenced tools have appropriate permissions for their risk tier
        for tool in &skill.tools {
            if (tool.safety_tier.clone() as u8) > (skill.manifest.risk_tier.clone() as u8) {
                return Err(SkillsError::Validation(format!(
                    "Tool {} has higher risk tier than skill",
                    tool.id
                )));
            }
        }

        Ok(())
    }

    async fn verify_signature(&self, skill: &Skill) -> Result<(), SkillsError> {
        if skill.manifest.signature.manifest_sig.trim().is_empty() {
            return Err(SkillsError::SignatureVerification(
                "Signature is empty".to_string(),
            ));
        }
        if skill.manifest.signature.bundle_hash.trim().is_empty() {
            return Err(SkillsError::SignatureVerification(
                "Bundle hash is empty".to_string(),
            ));
        }

        let digest = compute_bundle_digest(skill)?;
        let expected_hash = HEXLOWER.encode(&digest);
        let provided_hash = normalize_bundle_hash(&skill.manifest.signature.bundle_hash)
            .map_err(|err| SkillsError::SignatureVerification(err.to_string()))?;
        if expected_hash != provided_hash {
            return Err(SkillsError::SignatureVerification(
                "bundle_hash does not match canonical skill payload".to_string(),
            ));
        }

        let publisher_key = self
            .storage
            .get_publisher_key(
                &skill.manifest.publisher.publisher_id,
                &skill.manifest.publisher.public_key_id,
            )
            .await?
            .ok_or_else(|| {
                SkillsError::SignatureVerification(format!(
                    "Publisher {} key {} not found",
                    skill.manifest.publisher.publisher_id, skill.manifest.publisher.public_key_id
                ))
            })?;

        if publisher_key.revoked {
            return Err(SkillsError::SignatureVerification(format!(
                "Publisher {} key {} is revoked",
                skill.manifest.publisher.publisher_id, skill.manifest.publisher.public_key_id
            )));
        }

        let public_key_bytes = decode_base64(&publisher_key.public_key).map_err(|_| {
            SkillsError::SignatureVerification("Invalid publisher public key".to_string())
        })?;
        if public_key_bytes.len() != 32 {
            return Err(SkillsError::SignatureVerification(
                "Publisher public key must be 32 bytes (ed25519)".to_string(),
            ));
        }

        let signature_bytes =
            decode_base64(&skill.manifest.signature.manifest_sig).map_err(|_| {
                SkillsError::SignatureVerification("Invalid signature encoding".to_string())
            })?;
        if signature_bytes.len() != 64 {
            return Err(SkillsError::SignatureVerification(
                "Signature must be 64 bytes (ed25519)".to_string(),
            ));
        }

        let verifier = signature::UnparsedPublicKey::new(&signature::ED25519, public_key_bytes);
        verifier.verify(&digest, &signature_bytes).map_err(|_| {
            SkillsError::SignatureVerification("Signature verification failed".to_string())
        })?;

        Ok(())
    }

    pub async fn get_skill(
        &self,
        skill_id: String,
        version: Option<String>,
    ) -> Result<Option<Skill>, SkillsError> {
        self.storage.get_skill(&skill_id, version.as_deref()).await
    }

    pub async fn list_skills(&self) -> Result<Vec<Skill>, SkillsError> {
        self.storage.list_skills().await
    }

    pub async fn list_skill_versions(&self, skill_id: String) -> Result<Vec<String>, SkillsError> {
        self.storage.list_skill_versions(&skill_id).await
    }

    pub async fn enable_skill(&self, skill_id: String, version: String) -> Result<(), SkillsError> {
        self.storage.enable_skill(&skill_id, &version).await?;

        // Emit skill enable event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "SkillEnable".to_string(),
            session_id: "system".to_string(),
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(),
            role: "skill_registry".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "skill_id": skill_id,
                "version": version
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(())
    }

    pub async fn disable_skill(
        &self,
        skill_id: String,
        version: String,
    ) -> Result<(), SkillsError> {
        self.storage.disable_skill(&skill_id, &version).await?;

        // Emit skill disable event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "SkillDisable".to_string(),
            session_id: "system".to_string(),
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(),
            role: "skill_registry".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "skill_id": skill_id,
                "version": version
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(())
    }

    pub async fn revoke_skill(
        &self,
        skill_id: String,
        version: Option<String>,
    ) -> Result<(), SkillsError> {
        self.storage
            .revoke_skill(&skill_id, version.as_deref())
            .await?;

        // Emit skill revoke event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "SkillRevoke".to_string(),
            session_id: "system".to_string(),
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(),
            role: "skill_registry".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "skill_id": skill_id,
                "version": version
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(())
    }

    pub async fn validate_input(
        &self,
        skill_id: String,
        input: &serde_json::Value,
    ) -> Result<(), SkillsError> {
        if let Some(skill) = self.storage.get_skill(&skill_id, None).await? {
            // In a real implementation, we would validate against the JSON schema
            // For now, we'll just do basic validation based on the schema structure
            // This is a simplified validation - a real implementation would use a JSON Schema validator

            // Check required fields based on schema (simplified)
            if let Some(obj) = input.as_object() {
                if !skill.manifest.inputs.schema.is_empty() {
                    // This is where we would validate against the actual JSON schema
                    // For now, just return Ok as a placeholder
                    tracing::debug!("Input validation for skill {}: {:?}", skill_id, obj);
                }
            }
        } else {
            return Err(SkillsError::SkillNotFound(skill_id));
        }

        Ok(())
    }

    // Publisher key management
    pub async fn register_publisher_key(&self, key: PublisherKey) -> Result<(), SkillsError> {
        if key.publisher_id.trim().is_empty() {
            return Err(SkillsError::Validation(
                "Publisher id cannot be empty".to_string(),
            ));
        }
        if key.public_key_id.trim().is_empty() {
            return Err(SkillsError::Validation(
                "Public key id cannot be empty".to_string(),
            ));
        }
        if key.public_key.trim().is_empty() {
            return Err(SkillsError::Validation(
                "Public key cannot be empty".to_string(),
            ));
        }

        let public_key_bytes = decode_base64(&key.public_key)?;
        if public_key_bytes.len() != 32 {
            return Err(SkillsError::Validation(
                "Public key must be 32 bytes (ed25519)".to_string(),
            ));
        }

        if let Some(existing) = self
            .storage
            .get_publisher_key(&key.publisher_id, &key.public_key_id)
            .await?
        {
            if existing.revoked {
                return Err(SkillsError::Validation(format!(
                    "Publisher key {}:{} is revoked",
                    key.publisher_id, key.public_key_id
                )));
            }
            return Err(SkillsError::Validation(format!(
                "Publisher key {}:{} already exists",
                key.publisher_id, key.public_key_id
            )));
        }

        let mut key = key;
        if key.created_at == 0 {
            key.created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
        }
        self.storage.store_publisher_key(&key).await?;
        Ok(())
    }

    pub async fn get_publisher_key(
        &self,
        publisher_id: String,
        public_key_id: String,
    ) -> Result<Option<PublisherKey>, SkillsError> {
        self.storage
            .get_publisher_key(&publisher_id, &public_key_id)
            .await
    }

    pub async fn list_publisher_keys(
        &self,
        publisher_id: Option<String>,
    ) -> Result<Vec<PublisherKey>, SkillsError> {
        self.storage
            .list_publisher_keys(publisher_id.as_deref())
            .await
    }

    pub async fn revoke_publisher_key(
        &self,
        publisher_id: String,
        public_key_id: String,
    ) -> Result<(), SkillsError> {
        let existing = self
            .storage
            .get_publisher_key(&publisher_id, &public_key_id)
            .await?;
        if existing.is_none() {
            return Err(SkillsError::Validation(format!(
                "Publisher key {}:{} not found",
                publisher_id, public_key_id
            )));
        }
        self.storage
            .revoke_publisher_key(&publisher_id, &public_key_id)
            .await
    }

    // Channel management
    pub async fn set_channel_version(
        &self,
        skill_id: String,
        channel: Channel,
        version: String,
    ) -> Result<(), SkillsError> {
        self.storage
            .set_channel_version(&skill_id, &channel.to_string(), &version)
            .await?;

        // Emit channel update event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "SkillChannelUpdate".to_string(),
            session_id: "system".to_string(),
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(),
            role: "skill_registry".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "skill_id": skill_id,
                "channel": channel.to_string(),
                "version": version
            }),
        };

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(())
    }

    pub async fn get_channel_version(
        &self,
        skill_id: String,
        channel: Channel,
    ) -> Result<Option<String>, SkillsError> {
        self.storage
            .get_channel_version(&skill_id, &channel.to_string())
            .await
    }

    pub async fn get_enabled_skills(&self) -> Result<Vec<Skill>, SkillsError> {
        self.storage.get_enabled_skills().await
    }

    // Rollback functionality
    pub async fn rollback_skill(
        &self,
        skill_id: String,
        target_version: String,
    ) -> Result<(), SkillsError> {
        // Get all versions of the skill
        let versions = self.storage.list_skill_versions(&skill_id).await?;

        // Check if the target version exists
        if !versions.contains(&target_version) {
            return Err(SkillsError::Validation(format!(
                "Version {} does not exist for skill {}",
                target_version, skill_id
            )));
        }

        // Disable the current version
        if let Some(current_version) = self
            .get_channel_version(skill_id.clone(), Channel::Stable)
            .await?
        {
            self.disable_skill(skill_id.clone(), current_version)
                .await?;
        }

        // Enable the target version
        self.enable_skill(skill_id.clone(), target_version.clone())
            .await?;

        // Update the stable channel to point to the target version
        self.set_channel_version(skill_id, Channel::Stable, target_version)
            .await?;

        Ok(())
    }
}

// Helper functions for common skill operations
impl SkillRegistry {
    pub async fn create_jit_skill_menu(&self) -> Result<Vec<ToolDefinition>, SkillsError> {
        let skills = self.storage.get_enabled_skills().await?;
        let mut menu = Vec::new();

        for skill in skills {
            // Create a tool definition for each skill for the JIT menu
            let tool = ToolDefinition {
                id: skill.manifest.id.clone(),
                name: skill.manifest.name.clone(),
                description: skill.manifest.description.clone(),
                tool_type: allternit_tools_gateway::ToolType::Local, // Placeholder
                command: "".to_string(), // Would be the skill execution command
                endpoint: "".to_string(),
                input_schema: serde_json::from_str(&skill.manifest.inputs.schema)
                    .unwrap_or_else(|_| serde_json::json!({})),
                output_schema: serde_json::from_str(&skill.manifest.outputs.schema)
                    .unwrap_or_else(|_| serde_json::json!({})),
                side_effects: skill.manifest.side_effects.clone(),
                idempotency_behavior: "unknown".to_string(),
                retryable: true,
                failure_classification: "transient".to_string(),
                safety_tier: skill.manifest.risk_tier.clone(),
                resource_limits: allternit_tools_gateway::ResourceLimits {
                    cpu: skill
                        .manifest
                        .runtime
                        .resources
                        .as_ref()
                        .and_then(|r| r.cpu.clone()),
                    memory: skill
                        .manifest
                        .runtime
                        .resources
                        .as_ref()
                        .and_then(|r| r.memory.clone()),
                    network: match &skill.manifest.environment.network {
                        NetworkAccess::None => allternit_tools_gateway::NetworkAccess::None,
                        NetworkAccess::DomainAllowlist(list) => {
                            allternit_tools_gateway::NetworkAccess::DomainAllowlist(list.clone())
                        }
                        NetworkAccess::Unrestricted => {
                            allternit_tools_gateway::NetworkAccess::Unrestricted
                        }
                    },
                    filesystem: match &skill.manifest.environment.filesystem {
                        FilesystemAccess::None => allternit_tools_gateway::FilesystemAccess::None,
                        FilesystemAccess::Allowlist(list) => {
                            allternit_tools_gateway::FilesystemAccess::Allowlist(list.clone())
                        }
                        FilesystemAccess::ReadWrite(list) => {
                            allternit_tools_gateway::FilesystemAccess::ReadWrite(list.clone())
                        }
                    },
                    time_limit: skill.manifest.runtime.timeouts.total.unwrap_or(300), // 5 minutes default
                },
                subprocess: None,
            };
            menu.push(tool);
        }

        Ok(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_messaging::MessagingSystem;
    use allternit_policy::PolicyEngine;
    use allternit_tools_gateway::ToolGateway;
    use data_encoding::BASE64;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use sqlx::SqlitePool;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_skill_registry() {
        // Create a temporary SQLite database file
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        let temp_path = format!("/tmp/test_skills_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        // Create a temporary SQLite database for messaging
        let temp_msg_db = NamedTempFile::new().unwrap();
        let msg_db_url = format!("sqlite://{}", temp_msg_db.path().to_string_lossy());
        let msg_pool = SqlitePool::connect(&msg_db_url).await.unwrap();

        let messaging_system = Arc::new(
            MessagingSystem::new_with_storage(history_ledger.clone(), msg_pool)
                .await
                .unwrap(),
        );
        let policy_engine = Arc::new(PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let tool_gateway = Arc::new(ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        let skill_registry = SkillRegistry::new_with_storage(
            history_ledger,
            messaging_system,
            policy_engine,
            tool_gateway,
            pool,
        )
        .await
        .unwrap();

        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public_key = BASE64.encode(keypair.public_key().as_ref());

        // Register a publisher key
        let publisher_key = PublisherKey {
            publisher_id: "test_publisher".to_string(),
            public_key_id: "key1".to_string(),
            public_key,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            revoked: false,
            revoked_at: None,
        };
        skill_registry
            .register_publisher_key(publisher_key)
            .await
            .unwrap();

        // Create a simple skill for testing
        let manifest = SkillManifest {
            id: "test.skill.example".to_string(),
            name: "Test Skill".to_string(),
            version: "1.0.0".to_string(),
            description: "A test skill".to_string(),
            author: "Test Author".to_string(),
            license: "MIT".to_string(),
            tags: vec!["test".to_string(), "example".to_string()],
            homepage: Some("https://example.com".to_string()),
            repository: Some("https://github.com/example/test-skill".to_string()),

            inputs: SkillIO {
                schema: r#"{"type": "object", "properties": {"input": {"type": "string"}}}"#
                    .to_string(),
                examples: Some(vec![serde_json::json!({"input": "test"})]),
            },
            outputs: SkillIO {
                schema: r#"{"type": "object", "properties": {"output": {"type": "string"}}}"#
                    .to_string(),
                examples: Some(vec![serde_json::json!({"output": "result"})]),
            },

            runtime: SkillRuntime {
                mode: RuntimeMode::Sandbox,
                timeouts: SkillTimeouts {
                    per_step: Some(60),
                    total: Some(300),
                },
                resources: Some(ResourceHints {
                    cpu: Some("100m".to_string()),
                    gpu: None,
                    memory: Some("128Mi".to_string()),
                }),
            },

            environment: SkillEnvironment {
                allowed_envs: vec![Environment::Dev, Environment::Stage],
                network: NetworkAccess::DomainAllowlist(vec!["api.example.com".to_string()]),
                filesystem: FilesystemAccess::Allowlist(vec!["/tmp".to_string()]),
            },

            side_effects: vec!["read".to_string()],

            risk_tier: SafetyTier::T0,
            required_permissions: vec!["perm_t0_read".to_string()],
            requires_policy_gate: true,
            publisher: PublisherInfo {
                publisher_id: "test_publisher".to_string(),
                public_key_id: "key1".to_string(),
            },
            signature: SignatureInfo {
                manifest_sig: "test_signature".to_string(),
                bundle_hash: "test_hash".to_string(),
            },
        };

        let workflow = SkillWorkflow {
            nodes: vec![WorkflowNode {
                id: "observe".to_string(),
                name: "Observe Phase".to_string(),
                phase: WorkflowPhase::Observe,
                tool_binding: "echo_tool".to_string(),
                inputs: vec!["input".to_string()],
                outputs: vec!["observed_data".to_string()],
            }],
            edges: vec![],
            per_node_constraints: HashMap::new(),
            artifact_outputs: vec!["result".to_string()],
        };

        let tools = vec![ToolDefinition {
            id: "echo_tool".to_string(),
            name: "Echo Tool".to_string(),
            description: "Simple echo tool".to_string(),
            tool_type: allternit_tools_gateway::ToolType::Local,
            command: "echo".to_string(),
            endpoint: "".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                }
            }),
            output_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "output": {"type": "string"}
                }
            }),
            side_effects: vec!["read".to_string()],
            idempotency_behavior: "idempotent".to_string(),
            retryable: true,
            failure_classification: "transient".to_string(),
            safety_tier: SafetyTier::T0,
            resource_limits: allternit_tools_gateway::ResourceLimits {
                cpu: Some("100m".to_string()),
                memory: Some("64Mi".to_string()),
                network: allternit_tools_gateway::NetworkAccess::None,
                filesystem: allternit_tools_gateway::FilesystemAccess::None,
                time_limit: 10,
            },
            subprocess: None,
        }];

        let mut skill = Skill {
            manifest,
            workflow,
            tools,
            human_routing: "This is a test skill for demonstration".to_string(),
        };

        let bundle_digest = compute_bundle_digest(&skill).unwrap();
        let bundle_hash = compute_bundle_hash(&skill).unwrap();
        let signature = keypair.sign(&bundle_digest);
        skill.manifest.signature.manifest_sig = BASE64.encode(signature.as_ref());
        skill.manifest.signature.bundle_hash = bundle_hash;

        // Register the skill
        let skill_id = skill_registry.register_skill(skill).await.unwrap();
        assert_eq!(skill_id, "test.skill.example");

        // Get the skill
        let retrieved_skill = skill_registry
            .get_skill("test.skill.example".to_string(), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved_skill.manifest.name, "Test Skill");

        // Test channel management
        skill_registry
            .set_channel_version(
                "test.skill.example".to_string(),
                Channel::Stable,
                "1.0.0".to_string(),
            )
            .await
            .unwrap();
        let version = skill_registry
            .get_channel_version("test.skill.example".to_string(), Channel::Stable)
            .await
            .unwrap();
        assert_eq!(version, Some("1.0.0".to_string()));

        // Create JIT menu
        let menu = skill_registry.create_jit_skill_menu().await.unwrap();
        assert_eq!(menu.len(), 1);
        assert_eq!(menu[0].name, "Test Skill");

        // Test publisher key management
        skill_registry
            .revoke_publisher_key("test_publisher".to_string(), "key1".to_string())
            .await
            .unwrap();
        let key = skill_registry
            .get_publisher_key("test_publisher".to_string(), "key1".to_string())
            .await
            .unwrap()
            .unwrap();
        assert!(key.revoked);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
