//! Provider Factory
//!
//! Factory for creating VerificationProvider instances based on configuration.

use crate::verification::providers::{FileBasedProvider, GrpcProvider};
use crate::verification::types::{GrpcConfig, ProviderError};
use crate::verification::VerificationProvider;
use std::path::PathBuf;
use std::sync::Arc;

/// Factory for creating verification providers
pub struct ProviderFactory;

impl ProviderFactory {
    /// Create a verification provider based on configuration
    ///
    /// # Arguments
    ///
    /// * `provider_type` - The type of provider to create
    /// * `config` - Provider configuration
    ///
    /// # Returns
    ///
    /// A boxed verification provider trait object
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use allternit_commrails::verification::ProviderFactory;
    /// use std::path::PathBuf;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Create a file-based provider
    /// let config = allternit_commrails::verification::types::FileBasedConfig {
    ///     evidences_dir: PathBuf::from(".allternit/evidence"),
    ///     timeout_secs: 30,
    ///     poll_interval_ms: 100,
    /// };
    /// let provider = ProviderFactory::create_file_based_with_config(config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn create_file_based_with_config(
        config: crate::verification::types::FileBasedConfig,
    ) -> Result<SharedProvider, ProviderError> {
        let provider = FileBasedProvider::with_config(config);
        Ok(Arc::new(provider))
    }

    pub fn create_grpc_with_config(
        config: GrpcConfig,
    ) -> Result<SharedProvider, ProviderError> {
        // Note: GrpcProvider needs to be connected asynchronously
        // This factory method creates a disconnected provider
        // Call connect() before use
        let provider = GrpcProvider::new(config);
        Ok(Arc::new(provider))
    }

    /// Create a file-based provider with explicit workspace root
    ///
    /// # Arguments
    ///
    /// * `evidences_dir` - Directory where evidence files are stored
    /// * `workspace_root` - Root directory of the workspace
    /// * `timeout_secs` - Timeout in seconds for waiting for evidence
    pub fn create_file_based(
        evidences_dir: impl Into<PathBuf>,
        workspace_root: impl Into<PathBuf>,
        timeout_secs: u64,
    ) -> Result<SharedProvider, ProviderError> {
        let file_config = crate::verification::types::FileBasedConfig {
            evidences_dir: evidences_dir.into(),
            timeout_secs,
            poll_interval_ms: 100,
        };

        let provider = FileBasedProvider::new(file_config, workspace_root);
        Ok(Arc::new(provider))
    }

    /// Create a gRPC provider (not connected)
    ///
    /// # Arguments
    ///
    /// * `endpoint` - gRPC endpoint URL (e.g., "http://localhost:50051")
    /// * `timeout_secs` - Timeout in seconds for RPC calls
    pub fn create_grpc(
        endpoint: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<SharedProvider, ProviderError> {
        let grpc_config = GrpcConfig {
            endpoint: endpoint.into(),
            timeout_secs,
        };

        let provider = GrpcProvider::new(grpc_config);
        Ok(Arc::new(provider))
    }

    /// Create a provider from environment configuration
    ///
    /// Checks environment variables:
    /// - `Allternit_VERIFICATION_PROVIDER`: "file" or "grpc"
    /// - `Allternit_VERIFICATION_EVIDENCES_DIR`: Directory for file-based provider
    /// - `Allternit_VERIFICATION_GRPC_ENDPOINT`: Endpoint for gRPC provider
    /// - `Allternit_VERIFICATION_TIMEOUT`: Timeout in seconds (default: 30)
    pub fn from_env() -> Result<SharedProvider, ProviderError> {
        use std::env;

        let provider_type = env::var("Allternit_VERIFICATION_PROVIDER")
            .unwrap_or_else(|_| "file".to_string())
            .to_lowercase();

        let timeout_secs = env::var("Allternit_VERIFICATION_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        match provider_type.as_str() {
            "grpc" => {
                let endpoint = env::var("Allternit_VERIFICATION_GRPC_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:50051".to_string());
                Self::create_grpc(endpoint, timeout_secs)
            }
            "file" | "file_based" | "file-based" => {
                let evidences_dir = env::var("Allternit_VERIFICATION_EVIDENCES_DIR")
                    .unwrap_or_else(|_| ".allternit/evidence".to_string());
                let workspace_root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                Self::create_file_based(evidences_dir, workspace_root, timeout_secs)
            }
            _ => Err(ProviderError::Config(format!(
                "Unknown provider type: {}", provider_type
            ))),
        }
    }
}

/// Async variant of provider factory for providers that need async initialization
pub struct AsyncProviderFactory;

impl AsyncProviderFactory {
    /// Create and connect a gRPC provider
    ///
    /// This is the preferred method for creating gRPC providers as it
    /// establishes the connection immediately.
    pub async fn create_grpc_connected(
        endpoint: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<SharedProvider, ProviderError> {
        let grpc_config = GrpcConfig {
            endpoint: endpoint.into(),
            timeout_secs,
        };

        let mut provider = GrpcProvider::new(grpc_config);
        provider.connect().await?;
        Ok(Arc::new(provider))
    }

    /// Create a provider from environment with async initialization
    pub async fn from_env() -> Result<SharedProvider, ProviderError> {
        use std::env;

        let provider_type = env::var("Allternit_VERIFICATION_PROVIDER")
            .unwrap_or_else(|_| "file".to_string())
            .to_lowercase();

        let timeout_secs = env::var("Allternit_VERIFICATION_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        match provider_type.as_str() {
            "grpc" => {
                let endpoint = env::var("Allternit_VERIFICATION_GRPC_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:50051".to_string());
                Self::create_grpc_connected(endpoint, timeout_secs).await
            }
            "file" | "file_based" | "file-based" => {
                let evidences_dir = env::var("Allternit_VERIFICATION_EVIDENCES_DIR")
                    .unwrap_or_else(|_| ".allternit/evidence".to_string());
                let workspace_root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                ProviderFactory::create_file_based(evidences_dir, workspace_root, timeout_secs)
            }
            _ => Err(ProviderError::Config(format!(
                "Unknown provider type: {}", provider_type
            ))),
        }
    }
}

/// Thread-safe shared provider
pub type SharedProvider = Arc<dyn VerificationProvider + Send + Sync>;

/// Factory for creating shared (thread-safe) providers
pub struct SharedProviderFactory;

impl SharedProviderFactory {
    /// Create a shared file-based provider
    pub fn create_file_based(
        evidences_dir: impl Into<PathBuf>,
        workspace_root: impl Into<PathBuf>,
        timeout_secs: u64,
    ) -> Result<SharedProvider, ProviderError> {
        ProviderFactory::create_file_based(evidences_dir, workspace_root, timeout_secs)
    }

    /// Create a shared gRPC provider
    pub fn create_grpc(
        endpoint: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<SharedProvider, ProviderError> {
        ProviderFactory::create_grpc(endpoint, timeout_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::ProviderType;
    use tempfile::TempDir;

    #[test]
    fn test_create_file_based() {
        let temp_dir = TempDir::new().unwrap();
        let provider = ProviderFactory::create_file_based(
            ".allternit/evidence",
            temp_dir.path(),
            30,
        );
        
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().provider_type(), ProviderType::FileBased);
    }

    #[test]
    fn test_create_grpc() {
        let provider = ProviderFactory::create_grpc("http://localhost:50051", 30);
        
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().provider_type(), ProviderType::Grpc);
    }

    #[test]
    fn test_create_file_based_with_config() {
        let config = crate::verification::types::FileBasedConfig {
            evidences_dir: std::path::PathBuf::from(".allternit/evidence"),
            timeout_secs: 30,
            poll_interval_ms: 100,
        };
        let provider = ProviderFactory::create_file_based_with_config(config);
        
        assert!(provider.is_ok());
    }

    #[test]
    fn test_shared_provider_factory() {
        let temp_dir = TempDir::new().unwrap();
        let provider = SharedProviderFactory::create_file_based(
            ".allternit/evidence",
            temp_dir.path(),
            30,
        );
        
        assert!(provider.is_ok());
    }
}
