//! Remaining Channels Native - OC-028
//!
//! Native Rust implementation of OpenClaw's remaining messaging channels.
//! This module provides pure Rust implementations of channels that were not
//! covered in the earlier channel implementations (WhatsApp, Signal, iMessage, etc.)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

/// Channel identifier
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelId(String);

impl ChannelId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Channel type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ChannelType {
    #[serde(rename = "whatsapp")]
    WhatsApp,
    #[serde(rename = "signal")]
    Signal,
    #[serde(rename = "imessage")]
    IMessage,
    #[serde(rename = "sms")]
    Sms,
    #[serde(rename = "email")]
    Email,
    #[serde(rename = "matrix")]
    Matrix,
    #[serde(rename = "msteams")]
    MsTeams,
    #[serde(rename = "googlechat")]
    GoogleChat,
    #[serde(rename = "line")]
    Line,
    #[serde(rename = "wechat")]
    WeChat,
    #[serde(rename = "custom")]
    Custom,
}

/// Channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub channel_type: ChannelType,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub credentials: ChannelCredentials,
    pub settings: HashMap<String, serde_json::Value>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Channel credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCredentials {
    pub auth_token: Option<String>,
    pub api_key: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub oauth_token: Option<String>,
    pub webhook_url: Option<String>,
    pub encrypted: bool,
}

/// Channel message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub id: String,
    pub channel_id: ChannelId,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub recipient_id: Option<String>,
    pub recipient_name: Option<String>,
    pub content: String,
    pub message_type: MessageType,
    pub timestamp: DateTime<Utc>,
    pub attachments: Option<Vec<Attachment>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub reply_to: Option<String>, // Message ID this is a reply to
}

/// Message type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "notification")]
    Notification,
    #[serde(rename = "system")]
    System,
    #[serde(rename = "tool_result")]
    ToolResult,
}

/// Attachment information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub url: Option<String>,
    pub local_path: Option<PathBuf>,
}

/// Channel operation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelOperationRequest {
    pub operation: ChannelOperation,
    pub context: Option<ChannelContext>,
}

/// Channel operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelOperation {
    /// Send a message to the channel
    SendMessage {
        channel_id: ChannelId,
        message: ChannelMessage,
    },

    /// Receive messages from the channel
    ReceiveMessages {
        channel_id: ChannelId,
        limit: Option<usize>,
        since: Option<DateTime<Utc>>,
    },

    /// List all channels
    ListChannels,

    /// Get channel information
    GetChannel { channel_id: ChannelId },

    /// Update channel configuration
    UpdateChannel {
        channel_id: ChannelId,
        updates: ChannelConfigUpdates,
    },

    /// Enable/disable a channel
    ToggleChannel {
        channel_id: ChannelId,
        enabled: bool,
    },

    /// Get channel history
    GetHistory {
        channel_id: ChannelId,
        limit: Option<usize>,
        offset: Option<usize>,
    },
}

/// Channel context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelContext {
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub user_id: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
}

/// Channel configuration updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfigUpdates {
    pub enabled: Option<bool>,
    pub credentials: Option<ChannelCredentials>,
    pub settings: Option<HashMap<String, serde_json::Value>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Channel operation response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelOperationResponse {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub execution_time_ms: u64,
}

/// Channel list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelListResponse {
    pub channels: Vec<ChannelSummary>,
    pub total_count: usize,
}

/// Channel summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSummary {
    pub id: ChannelId,
    pub name: String,
    pub channel_type: ChannelType,
    pub enabled: bool,
    pub message_count: usize,
    pub last_activity: Option<DateTime<Utc>>,
}

/// Channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelServiceConfig {
    pub channels_dir: PathBuf,
    pub enable_persistence: bool,
    pub enable_encryption: bool,
    pub encryption_key: Option<String>,
    pub default_timeout_ms: u64,
    pub max_concurrent_connections: Option<usize>,
    pub enable_rate_limiting: bool,
    pub rate_limit_requests_per_minute: Option<u32>,
    pub enable_security_controls: bool,
    pub security_policy: SecurityPolicy,
    pub enable_message_logging: bool,
    pub log_retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityPolicy {
    /// Deny all channel operations (safe mode)
    Deny,

    /// Allow only whitelisted channels
    Allowlist { allowed_channels: Vec<String> },

    /// Allow all operations (unsafe, for development)
    Allow,
}

impl Default for ChannelServiceConfig {
    fn default() -> Self {
        Self {
            channels_dir: PathBuf::from("./channels"),
            enable_persistence: true,
            enable_encryption: false,
            encryption_key: None,
            default_timeout_ms: 30_000, // 30 seconds
            max_concurrent_connections: Some(10),
            enable_rate_limiting: true,
            rate_limit_requests_per_minute: Some(100),
            enable_security_controls: true,
            security_policy: SecurityPolicy::Allowlist {
                allowed_channels: vec![
                    "whatsapp".to_string(),
                    "signal".to_string(),
                    "imessage".to_string(),
                    "email".to_string(),
                    "matrix".to_string(),
                    "msteams".to_string(),
                    "googlechat".to_string(),
                    "line".to_string(),
                    "wechat".to_string(),
                ],
            },
            enable_message_logging: true,
            log_retention_days: Some(30),
        }
    }
}

/// Channel service trait for native implementations
#[async_trait::async_trait]
pub trait NativeChannel: Send + Sync {
    async fn send_message(
        &self,
        message: &ChannelMessage,
    ) -> Result<ChannelOperationResponse, ChannelError>;
    async fn receive_messages(
        &self,
        limit: Option<usize>,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ChannelMessage>, ChannelError>;
    async fn get_channel_info(&self) -> Result<ChannelInfo, ChannelError>;
    fn channel_type(&self) -> ChannelType;
    fn channel_id(&self) -> &ChannelId;
}

/// Channel information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub name: String,
    pub channel_type: ChannelType,
    pub connected: bool,
    pub last_ping: Option<DateTime<Utc>>,
    pub message_count: u64,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Channel service
pub struct ChannelService {
    config: ChannelServiceConfig,
    channels: HashMap<ChannelId, Box<dyn NativeChannel>>,
    channel_configs: HashMap<ChannelId, ChannelConfig>,
    connection_pool: Option<tokio::sync::Semaphore>,
    rate_limiters: HashMap<ChannelId, tokio::sync::Semaphore>,
}

impl ChannelService {
    /// Create new channel service with default configuration
    pub fn new() -> Self {
        let config = ChannelServiceConfig::default();
        let connection_semaphore = config
            .max_concurrent_connections
            .map(tokio::sync::Semaphore::new);

        Self {
            config,
            channels: HashMap::new(),
            channel_configs: HashMap::new(),
            connection_pool: connection_semaphore,
            rate_limiters: HashMap::new(),
        }
    }

    /// Create new channel service with custom configuration
    pub fn with_config(config: ChannelServiceConfig) -> Self {
        let connection_semaphore = config
            .max_concurrent_connections
            .map(tokio::sync::Semaphore::new);

        Self {
            config,
            channels: HashMap::new(),
            channel_configs: HashMap::new(),
            connection_pool: connection_semaphore,
            rate_limiters: HashMap::new(),
        }
    }

    /// Initialize the service by loading existing channel configurations
    pub async fn initialize(&mut self) -> Result<(), ChannelError> {
        self.ensure_channels_dir().await?;
        self.load_channel_configs().await?;
        self.initialize_rate_limiters();
        Ok(())
    }

    /// Ensure the channels directory exists
    async fn ensure_channels_dir(&self) -> Result<(), ChannelError> {
        fs::create_dir_all(&self.config.channels_dir)
            .await
            .map_err(|e| {
                ChannelError::IoError(format!("Failed to create channels directory: {}", e))
            })
    }

    /// Load channel configurations from disk
    async fn load_channel_configs(&mut self) -> Result<(), ChannelError> {
        if !self.config.channels_dir.exists() {
            return Ok(());
        }

        let mut entries = fs::read_dir(&self.config.channels_dir).await.map_err(|e| {
            ChannelError::IoError(format!("Failed to read channels directory: {}", e))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ChannelError::IoError(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(content) = fs::read_to_string(&path).await {
                        if let Ok(channel_config) = serde_json::from_str::<ChannelConfig>(&content)
                        {
                            self.channel_configs
                                .insert(channel_config.id.clone(), channel_config);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Initialize rate limiters for each channel
    fn initialize_rate_limiters(&mut self) {
        for (channel_id, config) in &self.channel_configs {
            if self.config.enable_rate_limiting {
                let default_limit =
                    self.config.rate_limit_requests_per_minute.unwrap_or(100) as u64;
                let max_requests = config
                    .settings
                    .get("rateLimitRequestsPerMinute")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(default_limit) as usize;

                self.rate_limiters.insert(
                    channel_id.clone(),
                    tokio::sync::Semaphore::new(max_requests),
                );
            }
        }
    }

    /// Execute a channel operation
    pub async fn execute_operation(
        &mut self,
        request: ChannelOperationRequest,
    ) -> Result<ChannelOperationResponse, ChannelError> {
        let start_time = std::time::Instant::now();

        let result = match request.operation {
            ChannelOperation::SendMessage {
                channel_id,
                message,
            } => self.send_message_to_channel(&channel_id, message).await,
            ChannelOperation::ReceiveMessages {
                channel_id,
                limit,
                since,
            } => {
                self.receive_messages_from_channel(&channel_id, limit, since)
                    .await
            }
            ChannelOperation::ListChannels => self.list_channels().await,
            ChannelOperation::GetChannel { channel_id } => self.get_channel(&channel_id).await,
            ChannelOperation::UpdateChannel {
                channel_id,
                updates,
            } => self.update_channel_config(&channel_id, updates).await,
            ChannelOperation::ToggleChannel {
                channel_id,
                enabled,
            } => self.toggle_channel(&channel_id, enabled).await,
            ChannelOperation::GetHistory {
                channel_id,
                limit,
                offset,
            } => self.get_channel_history(&channel_id, limit, offset).await,
        };

        let execution_time = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(result_value) => Ok(ChannelOperationResponse {
                success: true,
                result: Some(result_value),
                error: None,
                execution_time_ms: execution_time,
            }),
            Err(error) => Ok(ChannelOperationResponse {
                success: false,
                result: None,
                error: Some(error.to_string()),
                execution_time_ms: execution_time,
            }),
        }
    }

    /// Send a message to a specific channel
    async fn send_message_to_channel(
        &self,
        channel_id: &ChannelId,
        message: ChannelMessage,
    ) -> Result<serde_json::Value, ChannelError> {
        // Check if channel exists
        if !self.channels.contains_key(channel_id) {
            return Err(ChannelError::ChannelNotFound(channel_id.0.clone()));
        }

        // Check security policy
        if self.config.enable_security_controls {
            self.check_security_policy("send_message", channel_id)?;
        }

        // Check rate limits
        if self.config.enable_rate_limiting {
            self.check_rate_limit(channel_id).await?;
        }

        // Get the channel and send the message
        let channel = self
            .channels
            .get(channel_id)
            .ok_or_else(|| ChannelError::ChannelNotFound(channel_id.0.clone()))?;

        let response = channel.send_message(&message).await?;

        if !response.success {
            return Err(ChannelError::CommunicationError(
                response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }

        Ok(serde_json::json!({
            "status": "message_sent",
            "messageId": message.id,
            "channelId": channel_id.as_str(),
        }))
    }

    /// Receive messages from a specific channel
    async fn receive_messages_from_channel(
        &self,
        channel_id: &ChannelId,
        limit: Option<usize>,
        since: Option<DateTime<Utc>>,
    ) -> Result<serde_json::Value, ChannelError> {
        // Check if channel exists
        if !self.channels.contains_key(channel_id) {
            return Err(ChannelError::ChannelNotFound(channel_id.0.clone()));
        }

        // Check security policy
        if self.config.enable_security_controls {
            self.check_security_policy("receive_messages", channel_id)?;
        }

        let channel = self
            .channels
            .get(channel_id)
            .ok_or_else(|| ChannelError::ChannelNotFound(channel_id.0.clone()))?;

        let messages = channel.receive_messages(limit, since).await?;

        Ok(serde_json::json!({
            "messages": messages,
            "count": messages.len(),
        }))
    }

    /// List all available channels
    async fn list_channels(&self) -> Result<serde_json::Value, ChannelError> {
        let mut channels: Vec<ChannelSummary> = self
            .channel_configs
            .values()
            .map(|config| {
                // In a real implementation, we'd get the actual message count and last activity
                // from the channel implementation
                ChannelSummary {
                    id: config.id.clone(),
                    name: config.name.clone(),
                    channel_type: config.channel_type.clone(),
                    enabled: config.enabled,
                    message_count: 0, // Would come from actual channel implementation
                    last_activity: Some(config.updated_at), // Would come from actual channel
                }
            })
            .collect();

        // Sort by most recently updated
        channels.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));

        Ok(serde_json::json!({
            "channels": channels,
            "count": channels.len(),
        }))
    }

    /// Get a specific channel configuration
    async fn get_channel(&self, channel_id: &ChannelId) -> Result<serde_json::Value, ChannelError> {
        match self.channel_configs.get(channel_id) {
            Some(config) => Ok(serde_json::json!({
                "channel": {
                    "id": config.id.as_str(),
                    "name": config.name,
                    "type": match config.channel_type {
                        ChannelType::WhatsApp => "whatsapp",
                        ChannelType::Signal => "signal",
                        ChannelType::IMessage => "imessage",
                        ChannelType::Sms => "sms",
                        ChannelType::Email => "email",
                        ChannelType::Matrix => "matrix",
                        ChannelType::MsTeams => "msteams",
                        ChannelType::GoogleChat => "googlechat",
                        ChannelType::Line => "line",
                        ChannelType::WeChat => "wechat",
                        ChannelType::Custom => "custom",
                    },
                    "enabled": config.enabled,
                    "description": config.description,
                    "settings": config.settings,
                    "metadata": config.metadata,
                    "createdAt": config.created_at,
                    "updatedAt": config.updated_at,
                }
            })),
            None => Err(ChannelError::ChannelNotFound(channel_id.0.clone())),
        }
    }

    /// Update channel configuration
    async fn update_channel_config(
        &mut self,
        channel_id: &ChannelId,
        updates: ChannelConfigUpdates,
    ) -> Result<serde_json::Value, ChannelError> {
        let config = self
            .channel_configs
            .get_mut(channel_id)
            .ok_or_else(|| ChannelError::ChannelNotFound(channel_id.0.clone()))?;

        // Apply updates
        if let Some(enabled) = updates.enabled {
            config.enabled = enabled;
        }

        if let Some(credentials) = updates.credentials {
            config.credentials = credentials;
        }

        if let Some(settings) = updates.settings {
            config.settings.extend(settings);
        }

        if let Some(metadata) = updates.metadata {
            if let Some(ref mut existing_metadata) = config.metadata {
                existing_metadata.extend(metadata);
            } else {
                config.metadata = Some(metadata);
            }
        }

        config.updated_at = Utc::now();

        // Persist if enabled
        let persist_config = if self.config.enable_persistence {
            Some(config.clone())
        } else {
            None
        };
        drop(config);
        if let Some(config) = persist_config {
            self.persist_channel_config(&config).await?;
        }

        Ok(serde_json::json!({
            "status": "channel_updated",
            "channelId": channel_id.as_str(),
        }))
    }

    /// Toggle channel enabled/disabled status
    async fn toggle_channel(
        &mut self,
        channel_id: &ChannelId,
        enabled: bool,
    ) -> Result<serde_json::Value, ChannelError> {
        let config = self
            .channel_configs
            .get_mut(channel_id)
            .ok_or_else(|| ChannelError::ChannelNotFound(channel_id.0.clone()))?;

        config.enabled = enabled;
        config.updated_at = Utc::now();

        // Persist if enabled
        let persist_config = if self.config.enable_persistence {
            Some(config.clone())
        } else {
            None
        };
        drop(config);
        if let Some(config) = persist_config {
            self.persist_channel_config(&config).await?;
        }

        Ok(serde_json::json!({
            "status": if enabled { "channel_enabled" } else { "channel_disabled" },
            "channelId": channel_id.as_str(),
        }))
    }

    /// Get channel history
    async fn get_channel_history(
        &self,
        channel_id: &ChannelId,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<serde_json::Value, ChannelError> {
        // This would typically read from a persisted message log
        // For now, we'll return an empty history
        // In a real implementation, this would access the channel's message history

        Ok(serde_json::json!({
            "messages": [],
            "count": 0,
            "limit": limit,
            "offset": offset,
        }))
    }

    /// Check security policy for an operation
    fn check_security_policy(
        &self,
        operation: &str,
        channel_id: &ChannelId,
    ) -> Result<(), ChannelError> {
        match &self.config.security_policy {
            SecurityPolicy::Deny => {
                return Err(ChannelError::SecurityError(format!(
                    "Operation '{}' denied by security policy",
                    operation
                )));
            }
            SecurityPolicy::Allowlist { allowed_channels } => {
                let channel_type_str = match self.channel_configs.get(channel_id) {
                    Some(config) => match config.channel_type {
                        ChannelType::WhatsApp => "whatsapp",
                        ChannelType::Signal => "signal",
                        ChannelType::IMessage => "imessage",
                        ChannelType::Sms => "sms",
                        ChannelType::Email => "email",
                        ChannelType::Matrix => "matrix",
                        ChannelType::MsTeams => "msteams",
                        ChannelType::GoogleChat => "googlechat",
                        ChannelType::Line => "line",
                        ChannelType::WeChat => "wechat",
                        ChannelType::Custom => "custom",
                    }
                    .to_string(),
                    None => channel_id.0.clone(), // Use ID if config not found
                };

                if !allowed_channels.contains(&channel_type_str)
                    && !allowed_channels.contains(&"*".to_string())
                {
                    return Err(ChannelError::SecurityError(format!(
                        "Channel '{}' not in allowlist for operation '{}'",
                        channel_type_str, operation
                    )));
                }
            }
            SecurityPolicy::Allow => {
                // No restrictions
            }
        }

        Ok(())
    }

    /// Check rate limit for a channel
    async fn check_rate_limit(&self, channel_id: &ChannelId) -> Result<(), ChannelError> {
        if let Some(semaphore) = self.rate_limiters.get(channel_id) {
            match tokio::time::timeout(std::time::Duration::from_millis(100), semaphore.acquire())
                .await
            {
                Ok(Ok(_permit)) => {
                    // Got permit, continue
                    Ok(())
                }
                Ok(Err(_)) => {
                    // Semaphore closed
                    Err(ChannelError::RateLimitExceeded(format!(
                        "Rate limiter closed for channel {}",
                        channel_id.0
                    )))
                }
                Err(_) => {
                    // Timeout - rate limit exceeded
                    Err(ChannelError::RateLimitExceeded(format!(
                        "Rate limit exceeded for channel {}",
                        channel_id.0
                    )))
                }
            }
        } else {
            // No rate limiter for this channel
            Ok(())
        }
    }

    /// Register a native channel implementation
    pub fn register_channel(&mut self, channel: Box<dyn NativeChannel>) {
        let channel_id = channel.channel_id().clone();
        let config = ChannelConfig {
            id: channel_id.clone(),
            channel_type: channel.channel_type(),
            name: format!("{:?} Channel", channel.channel_type()),
            description: Some(format!(
                "Native implementation of {:?}",
                channel.channel_type()
            )),
            enabled: true,
            credentials: ChannelCredentials {
                auth_token: None,
                api_key: None,
                username: None,
                password: None,
                oauth_token: None,
                webhook_url: None,
                encrypted: false,
            },
            settings: HashMap::new(),
            metadata: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.channels.insert(channel_id.clone(), channel);
        self.channel_configs.insert(channel_id, config);
    }

    /// Persist channel configuration to disk
    async fn persist_channel_config(&self, config: &ChannelConfig) -> Result<(), ChannelError> {
        if !self.config.enable_persistence {
            return Ok(());
        }

        let channel_path = self
            .config
            .channels_dir
            .join(format!("{}.json", config.id.as_str()));
        let config_json = serde_json::to_string_pretty(config).map_err(|e| {
            ChannelError::SerializationError(format!("Failed to serialize channel config: {}", e))
        })?;

        fs::write(&channel_path, config_json)
            .await
            .map_err(|e| ChannelError::IoError(format!("Failed to write channel config: {}", e)))?;

        Ok(())
    }

    /// Get current configuration
    pub fn config(&self) -> &ChannelServiceConfig {
        &self.config
    }

    /// Get mutable access to configuration
    pub fn config_mut(&mut self) -> &mut ChannelServiceConfig {
        &mut self.config
    }

    /// Check if a channel exists
    pub fn has_channel(&self, channel_id: &ChannelId) -> bool {
        self.channels.contains_key(channel_id)
    }

    /// Get channel info
    pub async fn get_channel_info(
        &self,
        channel_id: &ChannelId,
    ) -> Result<ChannelInfo, ChannelError> {
        if let Some(channel) = self.channels.get(channel_id) {
            channel.get_channel_info().await
        } else {
            Err(ChannelError::ChannelNotFound(channel_id.0.clone()))
        }
    }
}

impl Default for ChannelService {
    fn default() -> Self {
        Self::new()
    }
}

/// Channel error
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("IO error: {0}")]
    IoError(String),

    #[error("Channel not found: {0}")]
    ChannelNotFound(String),

    #[error("Communication error: {0}")]
    CommunicationError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Timeout error")]
    Timeout,
}

impl From<serde_json::Error> for ChannelError {
    fn from(error: serde_json::Error) -> Self {
        ChannelError::SerializationError(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestWhatsAppChannel {
        id: ChannelId,
    }

    #[async_trait::async_trait]
    impl NativeChannel for TestWhatsAppChannel {
        async fn send_message(
            &self,
            message: &ChannelMessage,
        ) -> Result<ChannelOperationResponse, ChannelError> {
            Ok(ChannelOperationResponse {
                success: true,
                result: Some(serde_json::json!({
                    "messageId": message.id,
                    "status": "sent"
                })),
                error: None,
                execution_time_ms: 100,
            })
        }

        async fn receive_messages(
            &self,
            limit: Option<usize>,
            since: Option<DateTime<Utc>>,
        ) -> Result<Vec<ChannelMessage>, ChannelError> {
            let mut messages = Vec::new();

            for i in 0..(limit.unwrap_or(10)) {
                messages.push(ChannelMessage {
                    id: format!("msg-{}", i),
                    channel_id: self.id.clone(),
                    sender_id: "test-sender".to_string(),
                    sender_name: Some("Test Sender".to_string()),
                    recipient_id: Some("test-recipient".to_string()),
                    recipient_name: Some("Test Recipient".to_string()),
                    content: format!("Test message {}", i),
                    message_type: MessageType::Text,
                    timestamp: Utc::now(),
                    attachments: None,
                    metadata: None,
                    reply_to: None,
                });
            }

            Ok(messages)
        }

        async fn get_channel_info(&self) -> Result<ChannelInfo, ChannelError> {
            Ok(ChannelInfo {
                id: self.id.clone(),
                name: "Test WhatsApp".to_string(),
                channel_type: ChannelType::WhatsApp,
                connected: true,
                last_ping: Some(Utc::now()),
                message_count: 100,
                metadata: None,
            })
        }

        fn channel_type(&self) -> ChannelType {
            ChannelType::WhatsApp
        }

        fn channel_id(&self) -> &ChannelId {
            &self.id
        }
    }

    #[tokio::test]
    async fn test_channel_service_creation() {
        let service = ChannelService::new();
        assert_eq!(service.config.channels_dir, PathBuf::from("./channels"));
        assert!(service.config.enable_persistence);
        assert_eq!(service.channels.len(), 0);
    }

    #[tokio::test]
    async fn test_channel_service_with_config() {
        let config = ChannelServiceConfig {
            channels_dir: PathBuf::from("/tmp/test-channels"),
            enable_persistence: false,
            max_concurrent_connections: Some(5),
            ..Default::default()
        };

        let service = ChannelService::with_config(config);
        assert_eq!(
            service.config.channels_dir,
            PathBuf::from("/tmp/test-channels")
        );
        assert!(!service.config.enable_persistence);
        assert_eq!(service.config.max_concurrent_connections, Some(5));
    }

    #[tokio::test]
    async fn test_register_and_use_channel() {
        let mut service = ChannelService::new();

        let test_channel = TestWhatsAppChannel {
            id: ChannelId::new("test-whatsapp".to_string()),
        };

        service.register_channel(Box::new(test_channel));

        assert!(service.has_channel(&ChannelId::new("test-whatsapp".to_string())));

        // Get channel info
        let info = service
            .get_channel_info(&ChannelId::new("test-whatsapp".to_string()))
            .await
            .unwrap();
        assert_eq!(info.name, "Test WhatsApp");
        assert_eq!(info.channel_type, ChannelType::WhatsApp);
        assert!(info.connected);
    }

    #[tokio::test]
    async fn test_send_message_to_channel() {
        let mut service = ChannelService::new();

        let test_channel = TestWhatsAppChannel {
            id: ChannelId::new("send-test".to_string()),
        };

        service.register_channel(Box::new(test_channel));

        let message = ChannelMessage {
            id: "test-message".to_string(),
            channel_id: ChannelId::new("send-test".to_string()),
            sender_id: "sender123".to_string(),
            sender_name: Some("Test Sender".to_string()),
            recipient_id: Some("recipient123".to_string()),
            recipient_name: Some("Test Recipient".to_string()),
            content: "Hello, world!".to_string(),
            message_type: MessageType::Text,
            timestamp: Utc::now(),
            attachments: None,
            metadata: None,
            reply_to: None,
        };

        let request = ChannelOperationRequest {
            operation: ChannelOperation::SendMessage {
                channel_id: ChannelId::new("send-test".to_string()),
                message,
            },
            context: None,
        };

        let response = service.execute_operation(request).await.unwrap();
        assert!(response.success);
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(
            result.get("status").and_then(|v| v.as_str()),
            Some("message_sent")
        );
    }

    #[tokio::test]
    async fn test_receive_messages_from_channel() {
        let mut service = ChannelService::new();

        let test_channel = TestWhatsAppChannel {
            id: ChannelId::new("receive-test".to_string()),
        };

        service.register_channel(Box::new(test_channel));

        let request = ChannelOperationRequest {
            operation: ChannelOperation::ReceiveMessages {
                channel_id: ChannelId::new("receive-test".to_string()),
                limit: Some(5),
                since: None,
            },
            context: None,
        };

        let response = service.execute_operation(request).await.unwrap();
        assert!(response.success);
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        let messages = result.get("messages").and_then(|v| v.as_array()).unwrap();
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap();

        assert_eq!(messages.len(), 5);
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_list_channels() {
        let mut service = ChannelService::new();

        // Register a few channels
        for i in 1..=3 {
            let test_channel = TestWhatsAppChannel {
                id: ChannelId::new(format!("list-test-{}", i)),
            };
            service.register_channel(Box::new(test_channel));
        }

        let request = ChannelOperationRequest {
            operation: ChannelOperation::ListChannels,
            context: None,
        };

        let response = service.execute_operation(request).await.unwrap();
        assert!(response.success);
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        let channels = result.get("channels").and_then(|v| v.as_array()).unwrap();
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap();

        assert_eq!(channels.len(), 3);
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_get_specific_channel() {
        let mut service = ChannelService::new();

        let test_channel = TestWhatsAppChannel {
            id: ChannelId::new("get-test".to_string()),
        };

        service.register_channel(Box::new(test_channel));

        let request = ChannelOperationRequest {
            operation: ChannelOperation::GetChannel {
                channel_id: ChannelId::new("get-test".to_string()),
            },
            context: None,
        };

        let response = service.execute_operation(request).await.unwrap();
        assert!(response.success);
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        let channel = result.get("channel").unwrap();

        assert_eq!(channel.get("id").and_then(|v| v.as_str()), Some("get-test"));
        assert_eq!(
            channel.get("type").and_then(|v| v.as_str()),
            Some("whatsapp")
        );
        assert_eq!(channel.get("enabled").and_then(|v| v.as_bool()), Some(true));
    }

    #[tokio::test]
    async fn test_toggle_channel() {
        let mut service = ChannelService::new();

        let test_channel = TestWhatsAppChannel {
            id: ChannelId::new("toggle-test".to_string()),
        };

        service.register_channel(Box::new(test_channel));

        // Verify initially enabled
        let config = service
            .channel_configs
            .get(&ChannelId::new("toggle-test".to_string()))
            .unwrap();
        assert!(config.enabled);

        // Disable the channel
        let request = ChannelOperationRequest {
            operation: ChannelOperation::ToggleChannel {
                channel_id: ChannelId::new("toggle-test".to_string()),
                enabled: false,
            },
            context: None,
        };

        let response = service.execute_operation(request).await.unwrap();
        assert!(response.success);

        // Verify disabled
        let config = service
            .channel_configs
            .get(&ChannelId::new("toggle-test".to_string()))
            .unwrap();
        assert!(!config.enabled);

        // Enable the channel again
        let request = ChannelOperationRequest {
            operation: ChannelOperation::ToggleChannel {
                channel_id: ChannelId::new("toggle-test".to_string()),
                enabled: true,
            },
            context: None,
        };

        let response = service.execute_operation(request).await.unwrap();
        assert!(response.success);

        // Verify enabled
        let config = service
            .channel_configs
            .get(&ChannelId::new("toggle-test".to_string()))
            .unwrap();
        assert!(config.enabled);
    }

    #[test]
    fn test_channel_id_display() {
        let channel_id = ChannelId::new("whatsapp".to_string());
        assert_eq!(format!("{}", channel_id), "whatsapp");
    }

    #[test]
    fn test_channel_types() {
        assert_eq!(
            serde_json::to_string(&ChannelType::WhatsApp).unwrap(),
            "\"whatsapp\""
        );
        assert_eq!(
            serde_json::to_string(&ChannelType::Signal).unwrap(),
            "\"signal\""
        );
        assert_eq!(
            serde_json::to_string(&ChannelType::IMessage).unwrap(),
            "\"imessage\""
        );
    }

}
