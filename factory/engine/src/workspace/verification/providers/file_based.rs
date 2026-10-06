//! File-Based Verification Provider
//!
//! Polls `.allternit/evidence/{wih_id}.json` and `.allternit/evidence/{wih_id}.ready`
//! files to gather evidence from the TypeScript verification service.

use crate::verification::types::{
    Evidence, FileBasedConfig, ProviderError, ProviderType,
};
use crate::verification::VerificationProvider;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

/// File-based verification provider
///
/// This provider polls the filesystem for evidence files written by
/// the TypeScript gizzi-code verification service.
pub struct FileBasedProvider {
    config: FileBasedConfig,
    workspace_root: PathBuf,
}

impl FileBasedProvider {
    /// Create a new file-based provider
    pub fn new(config: FileBasedConfig, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            config,
            workspace_root: workspace_root.into(),
        }
    }

    /// Create a new file-based provider with default workspace
    pub fn with_config(config: FileBasedConfig) -> Self {
        // Default to current directory or detect from environment
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new(config, workspace_root)
    }

    /// Get the path to the evidence JSON file for a WIH
    fn evidence_path(&self, wih_id: &str) -> PathBuf {
        self.workspace_root
            .join(&self.config.evidences_dir)
            .join(format!("{}.json", wih_id))
    }

    /// Get the path to the ready marker file for a WIH
    fn ready_path(&self, wih_id: &str) -> PathBuf {
        self.workspace_root
            .join(&self.config.evidences_dir)
            .join(format!("{}.ready", wih_id))
    }

    /// Ensure the evidences directory exists
    async fn ensure_directory(&self) -> Result<(), ProviderError> {
        let dir = self.workspace_root.join(&self.config.evidences_dir);
        if !dir.exists() {
            debug!("Creating evidences directory: {:?}", dir);
            tokio::fs::create_dir_all(&dir).await.map_err(|e| {
                ProviderError::Internal(format!("Failed to create evidences directory: {}", e))
            })?;
        }
        Ok(())
    }

    /// Poll for the ready file with timeout
    async fn wait_for_ready(&self, wih_id: &str) -> Result<(), ProviderError> {
        let ready_path = self.ready_path(wih_id);
        let evidence_path = self.evidence_path(wih_id);
        let poll_interval = Duration::from_millis(self.config.poll_interval_ms);
        let timeout_duration = Duration::from_secs(self.config.timeout_secs);

        info!(
            wih_id = %wih_id,
            ready_path = %ready_path.display(),
            evidence_path = %evidence_path.display(),
            "Waiting for evidence files"
        );

        let wait_result = timeout(timeout_duration, async {
            loop {
                // Check if ready file exists
                if ready_path.exists() {
                    debug!(wih_id = %wih_id, "Ready file found");
                    // Also verify evidence file exists
                    if evidence_path.exists() {
                        return Ok(());
                    } else {
                        warn!(
                            wih_id = %wih_id,
                            "Ready file exists but evidence file is missing"
                        );
                    }
                }
                sleep(poll_interval).await;
            }
        })
        .await;

        match wait_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ProviderError::Timeout(self.config.timeout_secs)),
        }
    }

    /// Read and parse the evidence JSON file
    async fn read_evidence(&self, wih_id: &str) -> Result<Evidence, ProviderError> {
        let evidence_path = self.evidence_path(wih_id);

        debug!(
            wih_id = %wih_id,
            path = %evidence_path.display(),
            "Reading evidence file"
        );

        let content = tokio::fs::read_to_string(&evidence_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProviderError::NotFound(evidence_path.display().to_string())
            } else {
                ProviderError::Internal(format!("IO error: {}", e))
            }
        })?;

        let evidence: Evidence = serde_json::from_str(&content).map_err(|e| {
            ProviderError::Internal(format!(
                "Failed to parse evidence JSON for {}: {}",
                wih_id, e
            ))
        })?;

        Ok(evidence)
    }

    /// Clean up evidence files after reading (optional)
    pub async fn cleanup(&self, wih_id: &str) -> Result<(), ProviderError> {
        let evidence_path = self.evidence_path(wih_id);
        let ready_path = self.ready_path(wih_id);

        if evidence_path.exists() {
            tokio::fs::remove_file(&evidence_path).await.ok();
        }
        if ready_path.exists() {
            tokio::fs::remove_file(&ready_path).await.ok();
        }

        info!(wih_id = %wih_id, "Cleaned up evidence files");
        Ok(())
    }
}

#[async_trait::async_trait]
impl VerificationProvider for FileBasedProvider {
    async fn gather_evidence(&self, wih_id: &str) -> Result<Evidence, ProviderError> {
        // Validate WIH ID
        if wih_id.is_empty() {
            return Err(ProviderError::InvalidWih("WIH ID cannot be empty".to_string()));
        }

        // Ensure directory exists
        self.ensure_directory().await?;

        // Wait for ready file
        self.wait_for_ready(wih_id).await?;

        // Read and parse evidence
        let evidence = self.read_evidence(wih_id).await?;

        info!(
            wih_id = %wih_id,
            success = evidence.success,
            confidence = evidence.overall_confidence,
            artifact_count = evidence.artifacts.len(),
            "Evidence gathered successfully"
        );

        Ok(evidence)
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        // Check that the evidences directory is accessible
        let dir = self.workspace_root.join(&self.config.evidences_dir);

        // Try to create the directory if it doesn't exist
        if !dir.exists() {
            tokio::fs::create_dir_all(&dir).await.map_err(|e| {
                ProviderError::Internal(format!(
                    "Cannot create evidences directory at {}: {}",
                    dir.display(),
                    e
                ))
            })?;
        }

        // Check write permission by creating a test file
        let test_file = dir.join(".health_check");
        match tokio::fs::write(&test_file, b"test").await {
            Ok(_) => {
                tokio::fs::remove_file(&test_file).await.ok();
                Ok(())
            }
            Err(e) => Err(ProviderError::Internal(format!(
                "Cannot write to evidences directory: {}",
                e
            ))),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::FileBased
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_file_based_provider_paths() {
        let temp_dir = TempDir::new().unwrap();
        let config = FileBasedConfig {
            evidences_dir: std::path::PathBuf::from(".allternit/evidence"),
            timeout_secs: 5,
            poll_interval_ms: 10,
        };

        let provider = FileBasedProvider::new(config, temp_dir.path());

        assert_eq!(
            provider.evidence_path("test-wih"),
            temp_dir.path().join(".allternit/evidence/test-wih.json")
        );
        assert_eq!(
            provider.ready_path("test-wih"),
            temp_dir.path().join(".allternit/evidence/test-wih.ready")
        );
    }

    #[tokio::test]
    async fn test_health_check() {
        let temp_dir = TempDir::new().unwrap();
        let config = FileBasedConfig {
            evidences_dir: std::path::PathBuf::from(".allternit/evidence"),
            timeout_secs: 5,
            poll_interval_ms: 100,
        };

        let provider = FileBasedProvider::new(config, temp_dir.path());
        let result = provider.health_check().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gather_evidence_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let config = FileBasedConfig {
            evidences_dir: std::path::PathBuf::from(".allternit/evidence"),
            timeout_secs: 1, // Short timeout for test
            poll_interval_ms: 50,
        };

        let provider = FileBasedProvider::new(config, temp_dir.path());

        // Don't create any files - should timeout
        let result = provider.gather_evidence("non-existent").await;
        
        assert!(matches!(result, Err(ProviderError::Timeout(1))));
    }

    #[tokio::test]
    async fn test_gather_evidence_success() {
        let temp_dir = TempDir::new().unwrap();
        let evidence_dir = temp_dir.path().join(".allternit/evidence");
        tokio::fs::create_dir_all(&evidence_dir).await.unwrap();

        let config = FileBasedConfig {
            evidences_dir: std::path::PathBuf::from(".allternit/evidence"),
            timeout_secs: 5,
            poll_interval_ms: 10,
        };

        let provider = FileBasedProvider::new(config, temp_dir.path());
        let wih_id = "test-wih-123";

        // Create evidence file in background
        let evidence_json = serde_json::json!({
            "evidence_id": "ev-123",
            "wih_id": wih_id,
            "artifacts": [],
            "overall_confidence": 0.95,
            "success": true,
            "errors": []
        });

        let evidence_path = evidence_dir.join(format!("{}.json", wih_id));
        let ready_path = evidence_dir.join(format!("{}.ready", wih_id));

        // Write files after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tokio::fs::write(&evidence_path, evidence_json.to_string())
                .await
                .unwrap();
            tokio::fs::write(&ready_path, "ready").await.unwrap();
        });

        let result = provider.gather_evidence(wih_id).await;

        assert!(result.is_ok(), "gather_evidence failed: {:?}", result.err());
        let evidence = result.unwrap();
        assert_eq!(evidence.evidence_id, "ev-123");
        assert_eq!(evidence.wih_id, wih_id);
        assert!(evidence.success);
        assert_eq!(evidence.overall_confidence, 0.95);
    }

    #[tokio::test]
    async fn test_invalid_wih_id() {
        let temp_dir = TempDir::new().unwrap();
        let config = FileBasedConfig {
            evidences_dir: std::path::PathBuf::from(".allternit/evidence"),
            timeout_secs: 1,
            poll_interval_ms: 10,
        };

        let provider = FileBasedProvider::new(config, temp_dir.path());
        
        let result = provider.gather_evidence("").await;
        assert!(matches!(result, Err(ProviderError::InvalidWih(_))));
    }
}
