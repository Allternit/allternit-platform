use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Configuration for visual verification
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VisualConfig {
    /// Whether visual verification is enabled
    pub enabled: bool,
    /// The provider type to use for visual verification
    pub provider_type: ProviderType,
    /// Minimum confidence threshold (0.0 to 1.0)
    pub min_confidence: f64,
    /// Timeout in seconds for visual verification
    pub timeout_seconds: u64,
    /// Directory to store visual evidence artifacts
    pub evidence_dir: PathBuf,
}

impl Default for VisualConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_type: ProviderType::Grpc,
            min_confidence: 0.8,
            timeout_seconds: 30,
            evidence_dir: PathBuf::from(".allternit/visual_evidence"),
        }
    }
}

/// Provider types for visual verification
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum ProviderType {
    /// gRPC-based provider (TypeScript sidecar)
    Grpc,
    /// File-based provider (polling for evidence files)
    FileBased,
    /// HTTP/REST-based provider
    Http,
    /// Local provider for testing
    Local,
    /// Unknown provider type
    Unknown,
}

/// Configuration for gRPC provider
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GrpcConfig {
    /// gRPC endpoint URL
    pub endpoint: String,
    /// Timeout in seconds
    pub timeout_secs: u64,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:50051".to_string(),
            timeout_secs: 30,
        }
    }
}

/// Configuration for file-based provider
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileBasedConfig {
    /// Directory to poll for evidence files
    pub evidences_dir: PathBuf,
    /// Poll interval in milliseconds
    pub poll_interval_ms: u64,
    /// Timeout in seconds
    pub timeout_secs: u64,
}

impl Default for FileBasedConfig {
    fn default() -> Self {
        Self {
            evidences_dir: PathBuf::from(".allternit/evidence"),
            poll_interval_ms: 500,
            timeout_secs: 30,
        }
    }
}

/// Provider error types
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProviderError {
    /// Connection error
    Connection(String),
    /// gRPC error
    Grpc(String),
    /// Timeout error
    Timeout(u64),
    /// Internal error
    Internal(String),
    /// Development server unavailable
    DevServerUnavailable(String),
    /// Evidence not found
    NotFound(String),
    /// Invalid configuration
    Config(String),
    /// Provider not initialized
    NotInitialized,
    /// Invalid WIH ID
    InvalidWih(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Connection(msg) => write!(f, "Connection error: {}", msg),
            ProviderError::Grpc(msg) => write!(f, "gRPC error: {}", msg),
            ProviderError::Timeout(secs) => write!(f, "Timeout error: {}s", secs),
            ProviderError::Internal(msg) => write!(f, "Internal error: {}", msg),
            ProviderError::DevServerUnavailable(msg) => write!(f, "Dev server unavailable: {}", msg),
            ProviderError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ProviderError::Config(msg) => write!(f, "Config error: {}", msg),
            ProviderError::NotInitialized => write!(f, "Provider not initialized"),
            ProviderError::InvalidWih(msg) => write!(f, "Invalid WIH: {}", msg),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Artifact types
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ArtifactType {
    /// UI state screenshot
    UiState,
    /// Coverage map
    CoverageMap,
    /// Console output
    ConsoleOutput,
    /// Visual diff
    VisualDiff,
    /// Error state
    ErrorState,
    /// Performance chart
    PerformanceChart,
    /// Structure diagram
    StructureDiagram,
    /// Network trace
    NetworkTrace,
    /// Other/unknown artifact type
    Other(String),
    /// Unknown artifact type (for default)
    Unknown,
}

/// Image data for artifacts
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageData {
    /// Image data (bytes)
    #[serde(with = "serde_bytes")]
    pub image_data: Vec<u8>,
    /// Image format (png, jpg, etc.)
    pub format: String,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Optional file path
    pub file_path: Option<String>,
}

/// Text data for artifacts
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextData {
    /// Text content
    pub content: String,
    /// Encoding
    pub encoding: String,
    /// Optional line count
    pub line_count: Option<u32>,
}

/// JSON data for artifacts
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonData {
    /// JSON content as string
    pub json_content: String,
    /// Schema version
    pub schema_version: Option<String>,
}

/// Binary data for artifacts
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinaryData {
    /// Binary data (bytes)
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    /// MIME type
    pub mime_type: String,
    /// Size in bytes
    pub size_bytes: u64,
}

/// Artifact data types
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ArtifactData {
    /// Image data
    Image(ImageData),
    /// Text data
    Text(TextData),
    /// JSON data
    Json(JsonData),
    /// Binary data
    Binary(BinaryData),
}

/// Visual evidence artifact
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact type
    pub artifact_type: ArtifactType,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
    /// Timestamp when captured
    pub timestamp: String,
    /// Optional metadata JSON
    pub metadata_json: Option<String>,
    /// Artifact data
    pub data: ArtifactData,
}

/// Visual evidence artifact metadata (for file-based)
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvidenceArtifact {
    /// Artifact identifier
    pub id: String,
    /// Type of artifact (screenshot, recording, etc.)
    pub artifact_type: String,
    /// Path to the artifact file
    pub path: PathBuf,
    /// Timestamp when the artifact was captured
    pub captured_at: String,
    /// Confidence score for this specific artifact
    pub confidence: f64,
}

/// Evidence gathered during visual verification
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Evidence {
    /// Unique evidence ID
    pub evidence_id: String,
    /// WIH ID
    pub wih_id: String,
    /// Whether the verification was successful overall
    pub success: bool,
    /// Overall confidence score (0.0 to 1.0)
    pub overall_confidence: f64,
    /// List of artifacts collected
    pub artifacts: Vec<Artifact>,
    /// Optional provider ID
    pub provider_id: Option<String>,
    /// Optional capture timestamp
    pub captured_at: Option<String>,
    /// Optional completion timestamp
    pub completed_at: Option<String>,
    /// List of errors if any
    pub errors: Vec<String>,
    /// Optional capture duration in milliseconds
    pub capture_duration_ms: Option<u64>,
}

/// Factory for creating visual verification providers
#[derive(Clone, Debug)]
pub struct ProviderFactory {
    /// gRPC endpoint for the TypeScript sidecar
    pub grpc_endpoint: Option<String>,
    /// File-based configuration
    pub file_based_config: Option<FileBasedConfig>,
    /// Workspace root directory
    pub workspace_root: PathBuf,
}

impl ProviderFactory {
    /// Create a new provider factory
    pub fn new(
        grpc_endpoint: Option<String>,
        file_based_config: Option<FileBasedConfig>,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            grpc_endpoint,
            file_based_config,
            workspace_root: workspace_root.into(),
        }
    }
}

impl Default for ProviderFactory {
    fn default() -> Self {
        Self {
            grpc_endpoint: Some("http://localhost:50051".to_string()),
            file_based_config: None,
            workspace_root: PathBuf::from("."),
        }
    }
}

/// Trait for visual verification providers
#[async_trait]
pub trait VerificationProvider: Send + Sync {
    /// Gather visual evidence for a WIH
    async fn gather_evidence(&self, wih_id: &str) -> Result<Evidence, ProviderError>;

    /// Check if provider is healthy/available
    async fn health_check(&self) -> Result<(), ProviderError>;

    /// Get the provider type
    fn provider_type(&self) -> ProviderType;
}

// Type alias for backward compatibility
pub type VisualProvider = dyn VerificationProvider;
