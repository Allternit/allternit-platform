//! gRPC Verification Provider
//!
//! Connects to the TypeScript verification service using gRPC/tonic
//! to gather evidence for WIH validation.

use crate::verification::grpc_proto::verification::{
    verification_provider_client::VerificationProviderClient, Artifact as ProtoArtifact,
    EvidenceRequest,
};
use crate::verification::types::{
    Artifact, ArtifactData, ArtifactType, BinaryData, Evidence, GrpcConfig, ImageData, JsonData,
    ProviderError, ProviderType, TextData,
};
use crate::verification::VerificationProvider;
use async_trait::async_trait;
use std::time::Duration;
use tonic::{transport::Channel, Request};
use tracing::{debug, info, warn};

/// gRPC-based verification provider
///
/// Connects to the TypeScript verification service via gRPC
/// to gather evidence for WIH validation.
pub struct GrpcProvider {
    config: GrpcConfig,
    client: Option<VerificationProviderClient<Channel>>,
}

impl GrpcProvider {
    /// Create a new gRPC provider (client not connected yet)
    pub fn new(config: GrpcConfig) -> Self {
        Self {
            config,
            client: None,
        }
    }

    /// Connect to the gRPC service
    pub async fn connect(&mut self) -> Result<(), ProviderError> {
        let endpoint = self.config.endpoint.clone();
        debug!(endpoint = %endpoint, "Connecting to verification service");

        let channel = Channel::from_shared(endpoint.clone())
            .map_err(|e| ProviderError::Grpc(format!("Invalid endpoint '{}': {}", endpoint, e)))?
            .connect_timeout(Duration::from_secs(self.config.timeout_secs))
            .connect()
            .await
            .map_err(|e| ProviderError::DevServerUnavailable(format!(
                "Failed to connect to verification service at {}: {}",
                endpoint, e
            )))?;

        self.client = Some(VerificationProviderClient::new(channel));
        info!(endpoint = %endpoint, "Connected to verification service");
        Ok(())
    }

    /// Ensure gRPC client is connected
    async fn _ensure_connected(&mut self) -> Result<&mut VerificationProviderClient<Channel>, ProviderError> {
        if self.client.is_none() {
            self.connect().await?;
        }
        Ok(self.client.as_mut().unwrap())
    }

    /// Convert protobuf artifact type to Rust enum
    fn convert_artifact_type(proto_type: i32) -> ArtifactType {
        match proto_type {
            1 => ArtifactType::UiState,
            2 => ArtifactType::CoverageMap,
            3 => ArtifactType::ConsoleOutput,
            4 => ArtifactType::VisualDiff,
            5 => ArtifactType::Other("ErrorState".to_string()),
            6 => ArtifactType::Other("PerformanceChart".to_string()),
            7 => ArtifactType::Other("StructureDiagram".to_string()),
            8 => ArtifactType::Other("NetworkTrace".to_string()),
            _ => ArtifactType::Other("Unknown".to_string()),
        }
    }

    /// Convert protobuf artifact data to Rust ArtifactData
    fn convert_artifact_data(proto_artifact: &ProtoArtifact) -> Option<ArtifactData> {
        // The proto oneof generates a data field with the oneof variants
        if let Some(ref data) = proto_artifact.data {
            use crate::verification::grpc_proto::verification::artifact::Data;
            match data {
                Data::Image(img) => Some(ArtifactData::Image(ImageData {
                    image_data: img.image_data.clone(),
                    width: img.width,
                    height: img.height,
                    format: img.format.clone(),
                    file_path: if img.file_path.is_empty() {
                        None
                    } else {
                        Some(img.file_path.clone())
                    },
                })),
                Data::Text(txt) => Some(ArtifactData::Text(TextData {
                    content: txt.content.clone(),
                    encoding: txt.encoding.clone(),
                    line_count: if txt.line_count == 0 {
                        None
                    } else {
                        Some(txt.line_count)
                    },
                })),
                Data::Json(json) => Some(ArtifactData::Json(JsonData {
                    json_content: json.json_content.clone(),
                    schema_version: if json.schema_version.is_empty() {
                        None
                    } else {
                        Some(json.schema_version.clone())
                    },
                })),
                Data::Binary(bin) => Some(ArtifactData::Binary(BinaryData {
                    data: bin.data.clone(),
                    mime_type: bin.mime_type.clone(),
                    size_bytes: bin.size_bytes,
                })),
            }
        } else {
            warn!("Artifact has no data");
            None
        }
    }

    /// Convert protobuf artifact to Rust Artifact
    fn convert_artifact(proto_artifact: ProtoArtifact) -> Option<Artifact> {
        let data = Self::convert_artifact_data(&proto_artifact)?;
        
        Some(Artifact {
            artifact_type: Self::convert_artifact_type(proto_artifact.artifact_type),
            confidence: proto_artifact.confidence,
            timestamp: proto_artifact.timestamp.clone(),
            metadata_json: if proto_artifact.metadata_json.is_empty() {
                        None
                    } else {
                        Some(proto_artifact.metadata_json.clone())
                    },
            data,
        })
    }

    /// Convert protobuf evidence response to Rust Evidence
    fn convert_evidence_response(response: crate::verification::grpc_proto::verification::EvidenceResponse) -> Evidence {
        let artifacts: Vec<Artifact> = response
            .artifacts
            .into_iter()
            .filter_map(Self::convert_artifact)
            .collect();

        Evidence {
            evidence_id: response.evidence_id,
            wih_id: response.wih_id,
            artifacts,
            overall_confidence: response.overall_confidence,
            success: response.success,
            provider_id: if response.provider_id.is_empty() {
                None
            } else {
                Some(response.provider_id)
            },
            captured_at: if response.captured_at.is_empty() {
                None
            } else {
                Some(response.captured_at)
            },
            completed_at: if response.completed_at.is_empty() {
                None
            } else {
                Some(response.completed_at)
            },
            errors: response.errors,
            capture_duration_ms: if response.capture_duration_ms == 0 {
                None
            } else {
                Some(response.capture_duration_ms as u64)
            },
        }
    }
}

#[async_trait]
impl VerificationProvider for GrpcProvider {
    async fn health_check(&self) -> Result<(), ProviderError> {
        // Default health check returns Ok
        Ok(())
    }
    async fn gather_evidence(&self, wih_id: &str) -> Result<Evidence, ProviderError> {
        if wih_id.is_empty() {
            return Err(ProviderError::Internal("WIH ID cannot be empty".to_string()));
        }

        // Clone timeout before borrow
        let timeout_secs = self.config.timeout_secs;
        
        // Clone client to avoid mutable borrow requirement
        let mut client = self.client.clone().ok_or(ProviderError::NotInitialized)?;

        let request = Request::new(EvidenceRequest {
            wih_id: wih_id.to_string(),
            metadata: std::collections::HashMap::new(),
            timeout_ms: (timeout_secs * 1000) as u32,
            changed_files: String::new(),
            artifact_types: vec![], // Empty = all types
        });

        debug!(wih_id = %wih_id, "Sending GatherEvidence request");

        let response = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            client.gather_evidence(request),
        )
        .await
        .map_err(|_| ProviderError::Timeout(timeout_secs))?
        .map_err(|e| match e.code() {
            tonic::Code::Unavailable => ProviderError::DevServerUnavailable(e.to_string()),
            tonic::Code::InvalidArgument => ProviderError::InvalidWih(e.to_string()),
            tonic::Code::DeadlineExceeded => ProviderError::Timeout(timeout_secs),
            _ => ProviderError::Grpc(e.to_string()),
        })?;

        let evidence = Self::convert_evidence_response(response.into_inner());

        info!(
            wih_id = %wih_id,
            evidence_id = %evidence.evidence_id,
            success = evidence.success,
            confidence = evidence.overall_confidence,
            artifact_count = evidence.artifacts.len(),
            "Evidence gathered via gRPC"
        );

        Ok(evidence)
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Grpc
    }
}

/// Builder for GrpcProvider
pub struct GrpcProviderBuilder {
    endpoint: String,
    timeout_secs: u64,
}

impl GrpcProviderBuilder {
    /// Create a new builder with default endpoint
    pub fn new() -> Self {
        Self {
            endpoint: "http://localhost:50051".to_string(),
            timeout_secs: 30,
        }
    }

    /// Set the gRPC endpoint
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Set the timeout
    pub fn timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Build and connect the provider
    pub async fn build(self) -> Result<GrpcProvider, ProviderError> {
        let config = GrpcConfig {
            endpoint: self.endpoint,
            timeout_secs: self.timeout_secs,
        };
        let mut provider = GrpcProvider::new(config);
        provider.connect().await?;
        Ok(provider)
    }
}

impl Default for GrpcProviderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_artifact_type() {
        assert_eq!(GrpcProvider::convert_artifact_type(0), ArtifactType::Other("Unknown".to_string()));
        assert_eq!(GrpcProvider::convert_artifact_type(1), ArtifactType::UiState);
        assert_eq!(GrpcProvider::convert_artifact_type(2), ArtifactType::CoverageMap);
        assert_eq!(GrpcProvider::convert_artifact_type(3), ArtifactType::ConsoleOutput);
        assert_eq!(GrpcProvider::convert_artifact_type(4), ArtifactType::VisualDiff);
        assert_eq!(GrpcProvider::convert_artifact_type(5), ArtifactType::Other("ErrorState".to_string()));
        assert_eq!(GrpcProvider::convert_artifact_type(6), ArtifactType::Other("PerformanceChart".to_string()));
        assert_eq!(GrpcProvider::convert_artifact_type(7), ArtifactType::Other("StructureDiagram".to_string()));
        assert_eq!(GrpcProvider::convert_artifact_type(8), ArtifactType::Other("NetworkTrace".to_string()));
    }

    #[test]
    fn test_grpc_config() {
        let config = GrpcConfig {
            endpoint: "http://localhost:50051".to_string(),
            timeout_secs: 60,
        };
        assert_eq!(config.endpoint, "http://localhost:50051");
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn test_grpc_provider_builder() {
        let builder = GrpcProviderBuilder::new()
            .endpoint("http://test:8080")
            .timeout(45);
        
        assert_eq!(builder.endpoint, "http://test:8080");
        assert_eq!(builder.timeout_secs, 45);
    }
}
