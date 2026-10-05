//! Verification Providers
//!
//! Implementations of the VerificationProvider trait for different
//! communication mechanisms with the TypeScript gizzi-code service.

pub mod file_based;
pub mod grpc;

pub use file_based::FileBasedProvider;
pub use grpc::GrpcProvider;
