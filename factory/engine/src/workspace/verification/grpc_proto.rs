//! Generated gRPC client code for verification provider
//! 
//! This module contains the protobuf-generated code for communicating
//! with the TypeScript verification service.

pub mod verification {
    tonic::include_proto!("verification");
}

// Re-export client
pub use verification::verification_provider_client::VerificationProviderClient;

// Re-export request/response types
pub use verification::{
    EvidenceRequest,
    EvidenceResponse,
    Artifact,
    ArtifactType,
    ImageData,
    TextData,
    JsonData,
    BinaryData,
    HealthRequest,
    HealthResponse,
    ComponentHealth,
    CapabilitiesRequest,
    CapabilitiesResponse,
    ImageDimensions,
    VerificationError,
    VerificationErrorCode,
};
