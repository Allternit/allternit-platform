//! # Allternit SDK Transport
//!
//! Transport layer abstractions and types for the Allternit SDK.
//!
//! ## Overview
//!
//! This crate provides transport layer types and abstractions for
//! message passing across different communication channels in the Allternit
//! platform. It supports multiple transport protocols including
//! iMessage, SMS, Web, and System channels.
//!
//! The transport layer handles:
//! - Message envelope serialization and routing
//! - Transport protocol abstraction
//! - Message content type handling
//! - Source identification and tracking
//!
//! ## Key Concepts
//!
//! - **TransportEnvelope**: Universal message envelope for all transports
//! - **TransportSource**: Source of a message (iMessage, SMS, Web, System)
//! - **MessageContent**: Content types (Text, Media, StructuredCard)
//! - **TransportTraits**: Trait for transport implementations
//!
//! ## Example
//!
//! ```rust
//! use allternit_sdk_transport::{
//!     TransportEnvelope, TransportSource, MessageContent
//! };
//!
//! // Create an iMessage transport envelope
//! let envelope = TransportEnvelope::new_imessage(
//!     "msg-001".to_string(),
//!     "user@example.com".to_string(),
//!     "Hello from iMessage".to_string(),
//! );
//!
//! // Create an SMS transport envelope
//! let sms_envelope = TransportEnvelope::new_sms(
//!     "msg-002".to_string(),
//!     "+1234567890".to_string(),
//!     "+0987654321".to_string(),
//!     "Hello via SMS".to_string(),
//! );
//! ```

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

/// Universal message envelope for transport layer communication.
///
/// `TransportEnvelope` is the standard container for all messages
/// passing through the Allternit transport layer. It provides protocol-agnostic
/// routing, source identification, and extensible metadata storage.
///
/// # Examples
///
/// ```
/// use allternit_sdk_transport::{
///     TransportEnvelope, TransportSource, MessageContent
/// };
///
/// let envelope = TransportEnvelope {
///     id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
///     timestamp: 1704067200,
///     source: TransportSource::System,
///     sender_id: "system".to_string(),
///     recipient_id: "agent-001".to_string(),
///     content: MessageContent::Text("System notification".to_string()),
///     metadata: {
///         let mut m = std::collections::HashMap::new();
///         m.insert("priority".to_string(), serde_json::json!("high"));
///         m
///     },
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportEnvelope {
    /// Unique message identifier (UUID)
    pub id: String,

    /// Unix timestamp when the message was created
    pub timestamp: u64,

    /// Source transport type and metadata
    pub source: TransportSource,

    /// Sender identifier (user ID, phone number, session ID, etc.)
    pub sender_id: String,

    /// Recipient identifier (agent ID, service ID, etc.)
    pub recipient_id: String,

    /// Message content (text, media, or structured card)
    pub content: MessageContent,

    /// Additional transport-specific metadata
    pub metadata: HashMap<String, Value>,
}

impl TransportEnvelope {
    /// Creates a new iMessage transport envelope.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique message identifier
    /// * `sender` - Sender's Apple ID or phone number
    /// * `content` - Message text content
    ///
    /// # Returns
    ///
    /// A `TransportEnvelope` configured for iMessage transport
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::TransportEnvelope;
    ///
    /// let envelope = TransportEnvelope::new_imessage(
    ///     "msg-001".to_string(),
    ///     "user@icloud.com".to_string(),
    ///     "Hello!".to_string(),
    /// );
    ///
    /// assert_eq!(envelope.sender_id, "user@icloud.com");
    /// assert_eq!(envelope.recipient_id, "hive");
    /// ```
    pub fn new_imessage(id: String, sender: String, content: String) -> Self {
        Self {
            id,
            timestamp: Utc::now().timestamp() as u64,
            source: TransportSource::IMessage { apple_id: None },
            sender_id: sender,
            recipient_id: "hive".to_string(),
            content: MessageContent::Text(content),
            metadata: HashMap::new(),
        }
    }

    /// Creates a new SMS transport envelope.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique message identifier
    /// * `from` - Sender's phone number
    /// * `to` - Recipient's phone number
    /// * `content` - Message text content
    ///
    /// # Returns
    ///
    /// A `TransportEnvelope` configured for SMS transport
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::TransportEnvelope;
    ///
    /// let envelope = TransportEnvelope::new_sms(
    ///     "msg-002".to_string(),
    ///     "+1234567890".to_string(),
    ///     "+0987654321".to_string(),
    ///     "Hello via SMS".to_string(),
    /// );
    ///
    /// assert_eq!(envelope.sender_id, "+1234567890");
    /// ```
    pub fn new_sms(id: String, from: String, to: String, content: String) -> Self {
        Self {
            id,
            timestamp: Utc::now().timestamp() as u64,
            source: TransportSource::Sms {
                provider: "default".to_string(),
                number: from.clone(),
            },
            sender_id: from,
            recipient_id: to,
            content: MessageContent::Text(content),
            metadata: HashMap::new(),
        }
    }

    /// Creates a new Web transport envelope.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique message identifier
    /// * `session_id` - Web session identifier
    /// * `sender` - Sender identifier
    /// * `content` - Message content
    ///
    /// # Returns
    ///
    /// A `TransportEnvelope` configured for Web transport
    pub fn new_web(
        id: String,
        session_id: String,
        sender: String,
        content: MessageContent,
    ) -> Self {
        Self {
            id,
            timestamp: Utc::now().timestamp() as u64,
            source: TransportSource::Web { session_id },
            sender_id: sender,
            recipient_id: "hive".to_string(),
            content,
            metadata: HashMap::new(),
        }
    }

    /// Creates a new System transport envelope.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique message identifier
    /// * `sender` - Sender service identifier
    /// * `content` - Message content
    ///
    /// # Returns
    ///
    /// A `TransportEnvelope` configured for System transport
    pub fn new_system(id: String, sender: String, content: MessageContent) -> Self {
        Self {
            id,
            timestamp: Utc::now().timestamp() as u64,
            source: TransportSource::System,
            sender_id: sender,
            recipient_id: "hive".to_string(),
            content,
            metadata: HashMap::new(),
        }
    }

    /// Generates a new unique message ID.
    ///
    /// # Returns
    ///
    /// A new UUID v4 as a string
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::TransportEnvelope;
    ///
    /// let id = TransportEnvelope::generate_id();
    /// assert!(!id.is_empty());
    /// ```
    pub fn generate_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Returns the current Unix timestamp.
    ///
    /// # Returns
    ///
    /// Current time as Unix timestamp
    pub fn now_timestamp() -> u64 {
        Utc::now().timestamp() as u64
    }

    /// Adds metadata to the envelope.
    ///
    /// # Arguments
    ///
    /// * `key` - Metadata key
    /// * `value` - Metadata value
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::TransportEnvelope;
    ///
    /// let mut envelope = TransportEnvelope::new_imessage(
    ///     "msg-001".to_string(),
    ///     "sender".to_string(),
    ///     "Hello".to_string(),
    /// );
    ///
    /// envelope.add_metadata("priority".to_string(), serde_json::json!("high"));
    /// assert!(envelope.metadata.contains_key("priority"));
    /// ```
    pub fn add_metadata(&mut self, key: String, value: Value) {
        self.metadata.insert(key, value);
    }

    /// Gets metadata value by key.
    ///
    /// # Arguments
    ///
    /// * `key` - Metadata key
    ///
    /// # Returns
    ///
    /// `Some(&Value)` if the key exists, `None` otherwise
    pub fn get_metadata(&self, key: &str) -> Option<&Value> {
        self.metadata.get(key)
    }

    /// Returns true if the content is text.
    pub fn is_text(&self) -> bool {
        matches!(self.content, MessageContent::Text(_))
    }

    /// Returns true if the content is media.
    pub fn is_media(&self) -> bool {
        matches!(self.content, MessageContent::Media { .. })
    }

    /// Returns true if the content is a structured card.
    pub fn is_structured(&self) -> bool {
        matches!(self.content, MessageContent::StructuredCard(_))
    }

    /// Returns the text content if present.
    ///
    /// # Returns
    ///
    /// `Some(&String)` if content is text, `None` otherwise
    pub fn as_text(&self) -> Option<&String> {
        match &self.content {
            MessageContent::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// Source of a transport message.
///
/// `TransportSource` identifies the origin transport protocol
/// and provides protocol-specific metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TransportSource {
    /// iMessage transport (Apple Messages)
    IMessage {
        /// Optional Apple ID of the sender
        apple_id: Option<String>,
    },

    /// SMS transport
    Sms {
        /// SMS provider identifier
        provider: String,
        /// Phone number of the sender
        number: String,
    },

    /// Web transport (HTTP/WebSocket)
    Web {
        /// Web session identifier
        session_id: String,
    },

    /// System transport (internal messages)
    System,
}

impl TransportSource {
    /// Returns the transport type as a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::TransportSource;
    ///
    /// let imessage = TransportSource::IMessage { apple_id: None };
    /// assert_eq!(imessage.transport_type(), "imessage");
    ///
    /// let sms = TransportSource::Sms {
    ///     provider: "twilio".to_string(),
    ///     number: "+1234567890".to_string(),
    /// };
    /// assert_eq!(sms.transport_type(), "sms");
    ///
    /// let web = TransportSource::Web { session_id: "abc".to_string() };
    /// assert_eq!(web.transport_type(), "web");
    ///
    /// let system = TransportSource::System;
    /// assert_eq!(system.transport_type(), "system");
    /// ```
    pub fn transport_type(&self) -> &'static str {
        match self {
            TransportSource::IMessage { .. } => "imessage",
            TransportSource::Sms { .. } => "sms",
            TransportSource::Web { .. } => "web",
            TransportSource::System => "system",
        }
    }

    /// Returns true if this is an iMessage source.
    pub fn is_imessage(&self) -> bool {
        matches!(self, TransportSource::IMessage { .. })
    }

    /// Returns true if this is an SMS source.
    pub fn is_sms(&self) -> bool {
        matches!(self, TransportSource::Sms { .. })
    }

    /// Returns true if this is a Web source.
    pub fn is_web(&self) -> bool {
        matches!(self, TransportSource::Web { .. })
    }

    /// Returns true if this is a System source.
    pub fn is_system(&self) -> bool {
        matches!(self, TransportSource::System)
    }
}

/// Content types for transport messages.
///
/// `MessageContent` represents the different types of content
/// that can be transported through the Allternit system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MessageContent {
    /// Plain text content
    Text(String),

    /// Media content (image, video, audio)
    Media {
        /// URL to the media resource
        url: String,
        /// MIME type of the media
        mime_type: String,
    },

    /// Structured card content (UI component)
    StructuredCard(Value),
}

impl MessageContent {
    /// Creates a new text content.
    ///
    /// # Arguments
    ///
    /// * `text` - The text content
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::MessageContent;
    ///
    /// let content = MessageContent::text("Hello, world!");
    /// assert!(matches!(content, MessageContent::Text(_)));
    /// ```
    pub fn text(text: impl Into<String>) -> Self {
        MessageContent::Text(text.into())
    }

    /// Creates new media content.
    ///
    /// # Arguments
    ///
    /// * `url` - URL to the media resource
    /// * `mime_type` - MIME type of the media
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::MessageContent;
    ///
    /// let content = MessageContent::media("https://example.com/image.jpg", "image/jpeg");
    /// assert!(matches!(content, MessageContent::Media { .. }));
    /// ```
    pub fn media(url: impl Into<String>, mime_type: impl Into<String>) -> Self {
        MessageContent::Media {
            url: url.into(),
            mime_type: mime_type.into(),
        }
    }

    /// Creates new structured card content.
    ///
    /// # Arguments
    ///
    /// * `card` - Card data as JSON
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::MessageContent;
    ///
    /// let content = MessageContent::card(serde_json::json!({
    ///     "title": "My Card",
    ///     "content": "Card content"
    /// }));
    /// assert!(matches!(content, MessageContent::StructuredCard(_)));
    /// ```
    pub fn card(card: Value) -> Self {
        MessageContent::StructuredCard(card)
    }

    /// Returns the content type as a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::MessageContent;
    ///
    /// let text = MessageContent::text("Hello");
    /// assert_eq!(text.content_type(), "text");
    ///
    /// let media = MessageContent::media("url", "image/jpeg");
    /// assert_eq!(media.content_type(), "media");
    ///
    /// let card = MessageContent::card(serde_json::json!({}));
    /// assert_eq!(card.content_type(), "structured_card");
    /// ```
    pub fn content_type(&self) -> &'static str {
        match self {
            MessageContent::Text(_) => "text",
            MessageContent::Media { .. } => "media",
            MessageContent::StructuredCard(_) => "structured_card",
        }
    }

    /// Returns the text content if this is a text message.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Returns true if this content can be displayed as text.
    pub fn is_displayable_as_text(&self) -> bool {
        matches!(self, MessageContent::Text(_))
    }
}

/// Trait for transport implementations.
///
/// `Transport` defines the interface that all transport implementations
/// must provide for sending and receiving messages.
///
/// # Examples
///
/// ```rust,ignore
/// use allternit_sdk_transport::{Transport, TransportEnvelope};
/// use async_trait::async_trait;
///
/// struct MyTransport;
///
/// #[async_trait]
/// impl Transport for MyTransport {
///     async fn send(&self, envelope: TransportEnvelope) -> Result<(), TransportError> {
///         // Implementation
///         Ok(())
///     }
///
///     async fn receive(&self) -> Result<TransportEnvelope, TransportError> {
///         // Implementation
///         todo!()
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Sends a message envelope.
    ///
    /// # Arguments
    ///
    /// * `envelope` - The message to send
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or a `TransportError` if sending fails
    async fn send(&self, envelope: TransportEnvelope) -> Result<(), TransportError>;

    /// Receives a message envelope.
    ///
    /// This method blocks until a message is received.
    ///
    /// # Returns
    ///
    /// The received `TransportEnvelope`, or a `TransportError` on failure
    async fn receive(&self) -> Result<TransportEnvelope, TransportError>;

    /// Returns the transport type identifier.
    fn transport_type(&self) -> &'static str;

    /// Returns true if the transport is currently connected.
    async fn is_connected(&self) -> bool;
}

/// Errors that can occur during transport operations.
///
/// `TransportError` represents the various failure modes that
/// can occur when sending or receiving messages.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Connection error
    #[error("Connection error: {0}")]
    Connection(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Authentication error
    #[error("Authentication error: {0}")]
    Authentication(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded")]
    RateLimited,

    /// Timeout error
    #[error("Timeout after {0}ms")]
    Timeout(u64),

    /// Generic transport error
    #[error("Transport error: {0}")]
    Other(String),
}

/// Configuration for transport connections.
///
/// `TransportConfig` contains common configuration options
/// for transport implementations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Connection timeout in milliseconds
    pub connection_timeout_ms: u64,

    /// Request timeout in milliseconds
    pub request_timeout_ms: u64,

    /// Retry attempts for failed operations
    pub retry_attempts: u32,

    /// Retry delay in milliseconds
    pub retry_delay_ms: u64,

    /// Enable keepalive
    pub keepalive: bool,

    /// Additional transport-specific configuration
    pub extra: HashMap<String, Value>,
}

impl TransportConfig {
    /// Creates a default transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_transport::TransportConfig;
    ///
    /// let config = TransportConfig::default();
    /// assert_eq!(config.connection_timeout_ms, 5000);
    /// assert_eq!(config.retry_attempts, 3);
    /// ```
    pub fn default() -> Self {
        Self {
            connection_timeout_ms: 5000,
            request_timeout_ms: 30000,
            retry_attempts: 3,
            retry_delay_ms: 1000,
            keepalive: true,
            extra: HashMap::new(),
        }
    }

    /// Sets the connection timeout.
    pub fn with_connection_timeout(mut self, ms: u64) -> Self {
        self.connection_timeout_ms = ms;
        self
    }

    /// Sets the request timeout.
    pub fn with_request_timeout(mut self, ms: u64) -> Self {
        self.request_timeout_ms = ms;
        self
    }

    /// Sets the retry attempts.
    pub fn with_retry_attempts(mut self, attempts: u32) -> Self {
        self.retry_attempts = attempts;
        self
    }

    /// Adds extra configuration.
    pub fn with_extra(mut self, key: String, value: Value) -> Self {
        self.extra.insert(key, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test iMessage envelope creation
    #[test]
    fn test_new_imessage() {
        let envelope = TransportEnvelope::new_imessage(
            "msg-001".to_string(),
            "user@icloud.com".to_string(),
            "Hello!".to_string(),
        );

        assert_eq!(envelope.id, "msg-001");
        assert_eq!(envelope.sender_id, "user@icloud.com");
        assert_eq!(envelope.recipient_id, "hive");
        assert!(envelope.source.is_imessage());
        assert!(envelope.is_text());
    }

    /// Test SMS envelope creation
    #[test]
    fn test_new_sms() {
        let envelope = TransportEnvelope::new_sms(
            "msg-002".to_string(),
            "+1234567890".to_string(),
            "+0987654321".to_string(),
            "SMS message".to_string(),
        );

        assert_eq!(envelope.sender_id, "+1234567890");
        assert_eq!(envelope.recipient_id, "+0987654321");
        assert!(envelope.source.is_sms());
    }

    /// Test transport source types
    #[test]
    fn test_transport_source_types() {
        let imessage = TransportSource::IMessage { apple_id: None };
        assert_eq!(imessage.transport_type(), "imessage");
        assert!(imessage.is_imessage());
        assert!(!imessage.is_sms());

        let sms = TransportSource::Sms {
            provider: "twilio".to_string(),
            number: "+1234567890".to_string(),
        };
        assert_eq!(sms.transport_type(), "sms");
        assert!(sms.is_sms());

        let web = TransportSource::Web {
            session_id: "abc".to_string(),
        };
        assert_eq!(web.transport_type(), "web");
        assert!(web.is_web());

        let system = TransportSource::System;
        assert_eq!(system.transport_type(), "system");
        assert!(system.is_system());
    }

    /// Test message content types
    #[test]
    fn test_message_content() {
        let text = MessageContent::text("Hello");
        assert_eq!(text.content_type(), "text");
        assert!(text.is_displayable_as_text());
        assert_eq!(text.as_text(), Some("Hello"));

        let media = MessageContent::media("https://example.com/img.jpg", "image/jpeg");
        assert_eq!(media.content_type(), "media");
        assert!(!media.is_displayable_as_text());

        let card = MessageContent::card(serde_json::json!({"title": "Card"}));
        assert_eq!(card.content_type(), "structured_card");
    }

    /// Test envelope metadata
    #[test]
    fn test_envelope_metadata() {
        let mut envelope = TransportEnvelope::new_system(
            "msg-003".to_string(),
            "service".to_string(),
            MessageContent::text("Test"),
        );

        envelope.add_metadata("priority".to_string(), serde_json::json!("high"));
        envelope.add_metadata("retry_count".to_string(), serde_json::json!(3));

        assert_eq!(
            envelope.get_metadata("priority"),
            Some(&serde_json::json!("high"))
        );
        assert_eq!(
            envelope.get_metadata("retry_count"),
            Some(&serde_json::json!(3))
        );
        assert!(envelope.get_metadata("nonexistent").is_none());
    }

    /// Test transport config
    #[test]
    fn test_transport_config() {
        let config = TransportConfig::default()
            .with_connection_timeout(10000)
            .with_retry_attempts(5)
            .with_extra("custom".to_string(), serde_json::json!("value"));

        assert_eq!(config.connection_timeout_ms, 10000);
        assert_eq!(config.retry_attempts, 5);
        assert_eq!(
            config.extra.get("custom"),
            Some(&serde_json::json!("value"))
        );
    }
}
