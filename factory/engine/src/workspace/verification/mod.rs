//! Verification Provider System
//!
//! Rust side of the visual verification integration with TypeScript gizzi-code.
//! Provides abstractions for gathering evidence (screenshots, coverage, etc.)
//! from the verification service.
//!
//! # Architecture
//!
//! The verification system consists of:
//! - `VerificationProvider` trait: Core interface for evidence gathering
//! - `FileBasedProvider`: Polls filesystem for evidence files
//! - `GrpcProvider`: Communicates via gRPC with TypeScript service
//! - `ProviderFactory`: Creates configured provider instances

pub mod grpc_proto;
pub mod provider_factory;
pub mod providers;
pub mod types;

// Re-export commonly used types
pub use grpc_proto::{
    VerificationProviderClient,
    EvidenceRequest,
    EvidenceResponse,
    Artifact as ProtoArtifact,
    ArtifactType as ProtoArtifactType,
    HealthRequest,
    HealthResponse,
    CapabilitiesRequest,
    CapabilitiesResponse,
};

// Re-export factory types
pub use provider_factory::{AsyncProviderFactory, ProviderFactory, SharedProvider, SharedProviderFactory};

// Re-export provider implementations
pub use providers::{FileBasedProvider, GrpcProvider};

// Re-export types module
pub use types::{
    Artifact,
    ArtifactData,
    ArtifactType,
    BinaryData,
    Evidence,
    EvidenceArtifact,
    FileBasedConfig,
    GrpcConfig,
    ImageData,
    JsonData,
    ProviderError,
    ProviderFactory as TypesProviderFactory,
    ProviderType,
    TextData,
    VerificationProvider,
    VisualConfig,
};
