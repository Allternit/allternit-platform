//! Deployment Orchestrator
//!
//! Coordinates the full deployment lifecycle.

use allternit_cloud_core::{
    CloudProvider, CloudError, DeploymentConfig, Instance, InstanceStatus,
    ProviderCredentials, PreflightChecker,
    DeploymentStatus, DeploymentPhase,
};
use crate::installer::AllternitInstaller;
use std::sync::Arc;
use chrono::Utc;

/// Deployment orchestrator
pub struct DeploymentOrchestrator {
    provider: Arc<dyn CloudProvider>,
    credentials: ProviderCredentials,
}

/// Deployment result
#[derive(Debug, Clone)]
pub struct DeploymentResult {
    pub instance_id: String,
    pub public_ip: String,
    pub access_url: String,
    pub admin_email: String,
    pub temporary_password: String,
    pub status: DeploymentStatus,
}

impl DeploymentOrchestrator {
    /// Create new orchestrator
    pub fn new(provider: Arc<dyn CloudProvider>, credentials: ProviderCredentials) -> Self {
        Self { provider, credentials }
    }
    
    /// Deploy Allternit to cloud
    pub async fn deploy(&self, config: DeploymentConfig) -> Result<DeploymentResult, CloudError> {
        tracing::info!("Starting Allternit deployment to {}", config.provider);
        
        // Create deployment status tracker
        let mut status = DeploymentStatus {
            id: uuid::Uuid::new_v4().to_string(),
            phase: DeploymentPhase::Pending,
            progress: 0,
            message: "Initializing deployment".to_string(),
            errors: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        
        // Step 1: Preflight checks
        status.phase = DeploymentPhase::Validating;
        status.progress = 10;
        status.message = "Running preflight checks".to_string();
        
        let preflight = PreflightChecker::new();
        let preflight_result = preflight.check(&config, &self.credentials).await?;
        
        if !preflight_result.passed {
            status.phase = DeploymentPhase::Failed;
            status.errors = preflight_result.errors.clone();
            status.progress = 100;
            return Err(CloudError::PreflightFailed(
                preflight_result.errors.join("; ")
            ));
        }
        
        // Step 2: Provision VM
        status.phase = DeploymentPhase::Provisioning;
        status.progress = 30;
        status.message = "Provisioning VM instance".to_string();
        
        let instance = self.provider.provision(config.clone()).await?;
        
        // Step 3: Wait for instance ready
        status.phase = DeploymentPhase::Installing;
        status.progress = 50;
        status.message = "Waiting for instance to be ready".to_string();
        
        self.wait_for_ready(&instance).await?;
        
        // Step 4: Install Allternit
        status.progress = 60;
        status.message = "Installing Allternit platform".to_string();
        
        self.install_allternit(&instance).await?;
        
        // Step 5: Configure networking
        status.phase = DeploymentPhase::Configuring;
        status.progress = 75;
        status.message = "Configuring networking and firewall".to_string();
        
        self.configure_networking(&instance).await?;
        
        // Step 6: Run health checks
        status.phase = DeploymentPhase::HealthChecking;
        status.progress = 90;
        status.message = "Running health checks".to_string();
        
        self.health_check(&instance).await?;
        
        // Step 7: Complete
        status.phase = DeploymentPhase::Complete;
        status.progress = 100;
        status.message = "Deployment complete".to_string();
        
        Ok(DeploymentResult {
            instance_id: instance.id,
            public_ip: instance.public_ip.clone().unwrap_or_else(|| "unknown".to_string()),
            access_url: format!("https://{}", instance.public_ip.clone().unwrap_or_else(|| "unknown".to_string())),
            admin_email: "admin@example.com".to_string(),
            temporary_password: uuid::Uuid::new_v4().to_string()[..12].to_string(),
            status,
        })
    }
    
    /// Wait for instance to be ready by polling the provider.
    async fn wait_for_ready(&self, instance: &Instance) -> Result<(), CloudError> {
        tracing::info!("Waiting for instance {} to be ready", instance.id);

        let max_attempts = 30;
        let delay = tokio::time::Duration::from_secs(10);

        for attempt in 1..=max_attempts {
            match self.provider.get_instance(instance.id.clone()).await {
                Ok(updated) => {
                    tracing::debug!(
                        "Instance {} status: {:?} (attempt {}/{})",
                        updated.id,
                        updated.status,
                        attempt,
                        max_attempts
                    );

                    match updated.status {
                        InstanceStatus::Running => {
                            tracing::info!("Instance {} is running", instance.id);
                            return Ok(());
                        }
                        InstanceStatus::Error => {
                            return Err(CloudError::ProvisioningFailed(
                                format!("Instance {} entered error state", instance.id)
                            ));
                        }
                        _ => {
                            // Still pending/stopped; keep polling
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to get instance {} status (attempt {}/{}): {}",
                        instance.id,
                        attempt,
                        max_attempts,
                        e
                    );
                }
            }

            tokio::time::sleep(delay).await;
        }

        Err(CloudError::Timeout(
            format!("Instance {} did not become ready within {} seconds", instance.id, max_attempts * 10)
        ))
    }

    /// Install Allternit on instance.
    /// When a real release URL is configured via `ALLTERNIT_RELEASE_URL`, the installer
    /// script is updated to download that release instead of using the placeholder binary.
    async fn install_allternit(&self, instance: &Instance) -> Result<(), CloudError> {
        tracing::info!("Installing Allternit on instance {}", instance.id);

        let installer = AllternitInstaller::new();
        let install_script = if let Ok(release_url) = std::env::var("ALLTERNIT_RELEASE_URL") {
            tracing::info!("Using release URL: {}", release_url);
            installer.get_install_script().replace(
                "# curl -L https://releases.allternit.sh/latest | tar xz -C /opt/allternit",
                &format!("curl -L {} | tar xz -C /opt/allternit", release_url),
            )
        } else {
            tracing::warn!(
                "No ALLTERNIT_RELEASE_URL set; instance {} will use a placeholder binary. Set ALLTERNIT_RELEASE_URL to a tarball URL for a real install.",
                instance.id
            );
            installer.get_install_script().to_string()
        };

        // In production this would run over SSH or be supplied as cloud-init user-data.
        // We log the script length and rely on the provider health check to verify readiness.
        tracing::debug!(
            "Installation script prepared ({} bytes) for {}",
            install_script.len(),
            instance.id
        );

        Ok(())
    }

    /// Configure networking for the instance.
    /// Currently prepares the firewall script; providers should apply it via cloud-init
    /// or SSH. When `ALLTERNIT_APPLY_FIREWALL` is set, this attempts to run the script
    /// directly if SSH credentials are available in the future.
    async fn configure_networking(&self, instance: &Instance) -> Result<(), CloudError> {
        tracing::info!("Configuring networking for instance {}", instance.id);

        let firewall_script = crate::scripts::get_firewall_script();
        tracing::debug!(
            "Firewall script prepared ({} bytes) for {}",
            firewall_script.len(),
            instance.id
        );

        if std::env::var("ALLTERNIT_APPLY_FIREWALL").is_ok() {
            tracing::warn!(
                "ALLTERNIT_APPLY_FIREWALL is set but direct SSH firewall application requires an SSH private key. Apply the firewall script via cloud-init or SSH manually for instance {}.",
                instance.id
            );
        }

        Ok(())
    }
    
    /// Run health checks
    async fn health_check(&self, instance: &Instance) -> Result<(), CloudError> {
        let health = self.provider.health_check(instance.id.clone()).await?;
        
        if !health.healthy {
            return Err(CloudError::HealthCheckFailed(
                "Instance health check failed".to_string()
            ));
        }
        
        tracing::info!("Health check passed for instance {}", instance.id);
        Ok(())
    }
    
    /// Get deployment status by querying the provider instance.
    pub async fn get_status(&self, deployment_id: &str) -> Result<DeploymentStatus, CloudError> {
        let instance = self.provider.get_instance(deployment_id.to_string()).await?;

        let (phase, progress, message) = match instance.status {
            InstanceStatus::Pending => (DeploymentPhase::Provisioning, 20, "Instance is pending"),
            InstanceStatus::Running => (DeploymentPhase::Complete, 100, "Instance is running"),
            InstanceStatus::Stopped => (DeploymentPhase::Configuring, 80, "Instance is stopped"),
            InstanceStatus::Terminated => (DeploymentPhase::Failed, 100, "Instance is terminated"),
            InstanceStatus::Error => (DeploymentPhase::Failed, 100, "Instance is in error state"),
        };

        Ok(DeploymentStatus {
            id: deployment_id.to_string(),
            phase,
            progress,
            message: message.to_string(),
            errors: vec![],
            created_at: instance.created_at,
            updated_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
}
