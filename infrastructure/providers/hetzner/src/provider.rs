//! Hetzner Provider Implementation
//!
//! Real Hetzner Cloud API integration for automated deployments.

use crate::{HetznerClient, CreateServerRequest, HetznerError};
use allternit_cloud_ssh::SshExecutor;
use tracing::{info, warn};

/// Hetzner provider for automated deployments
pub struct HetznerProvider {
    client: HetznerClient,
    ssh_executor: SshExecutor,
}

impl HetznerProvider {
    /// Create a new Hetzner provider
    pub fn new(api_token: &str) -> Self {
        Self {
            client: HetznerClient::new(api_token),
            ssh_executor: SshExecutor::new(),
        }
    }
    
    /// Validate API credentials
    pub async fn validate_credentials(&self) -> Result<bool, HetznerError> {
        self.client.validate_token().await
    }
    
    /// Deploy Allternit to Hetzner Cloud
    pub async fn deploy(&self, config: &DeploymentConfig) -> Result<DeploymentResult, HetznerError> {
        info!("Deploying to Hetzner: {:?}", config);
        
        // Generate SSH keypair for this deployment
        let keypair = self.ssh_executor.generate_keypair()
            .map_err(|e| HetznerError::ApiError(format!("Failed to generate SSH key: {}", e)))?;
        
        info!("Generated SSH keypair for deployment");
        
        // Create server
        let server_request = CreateServerRequest {
            name: config.instance_name.clone(),
            server_type: config.instance_type_id.clone(),
            image: "ubuntu-22.04".to_string(),
            location: config.region_id.clone(),
            public_keys: Some(vec![keypair.public_key.clone()]),
            start_after_create: Some(true),
        };
        
        info!("Creating server: {:?}", server_request);
        let server = self.client.create_server(&server_request).await?;
        info!("Server created: {} ({})", server.name, server.id);
        
        // Wait for server to be ready
        info!("Waiting for server {} to be ready", server.id);
        self.wait_for_server_ready(server.id).await?;
        
        // Install gizzi-code via SSH (legacy path — no mesh enrollment; the
        // BYO-VPS wizard is the mesh-joined happy path).
        info!("Installing gizzi-code on {}", server.name);
        let install_result = self.ssh_executor.run_script(
            &server.public_net.ipv4.ip,
            22,
            "root",
            &keypair.private_key,
            &legacy_gizzi_install_script(),
        ).await;
        
        match install_result {
            Ok(output) => {
                info!("Allternit runtime installed successfully");
                info!("Installation output: {}", output.stdout);
            }
            Err(e) => {
                warn!("Allternit installation failed: {}", e);
                return Err(HetznerError::ApiError(format!("Installation failed: {}", e)));
            }
        }
        
        Ok(DeploymentResult {
            instance_id: server.id.to_string(),
            instance_ip: server.public_net.ipv4.ip,
            server_name: server.name,
            ssh_key: keypair.private_key,
        })
    }
    
    /// Wait for server to be ready
    async fn wait_for_server_ready(&self, server_id: i64) -> Result<(), HetznerError> {
        // Poll server status
        for attempt in 0..30 {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            
            let server = self.client.get_server(server_id).await?;
            
            if server.status == "running" {
                info!("Server {} is running", server_id);
                return Ok(());
            }
            
            info!("Server {} status: {} (attempt {}/{})", server_id, server.status, attempt + 1, 30);
        }
        
        Err(HetznerError::ApiError("Server did not become ready in time".to_string()))
    }
    
    /// Delete server
    pub async fn delete_server(&self, server_id: i64) -> Result<(), HetznerError> {
        info!("Deleting server {}", server_id);
        self.client.delete_server(server_id).await
    }
}

/// Deployment configuration
#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    pub instance_name: String,
    pub instance_type_id: String,
    pub region_id: String,
    pub storage_gb: i32,
    pub control_plane_url: String,
    pub deployment_token: String,
}

/// Deployment result
#[derive(Debug, Clone)]
pub struct DeploymentResult {
    pub instance_id: String,
    pub instance_ip: String,
    pub server_name: String,
    pub ssh_key: String,
}

/// Legacy install script for the old cowork deployment flow: checksum-pinned
/// gizzi-code release + systemd unit, no tailnet enrollment (this flow has no
/// Headscale preauth key; the BYO-VPS wizard handles the mesh-joined path).
/// Mirrors `cmd/allternit-hosted-runtime/Dockerfile` pins.
fn legacy_gizzi_install_script() -> String {
    const RELEASE: &str = "hosted-runtime-2026.07.16";
    const X64_SHA256: &str =
        "f1d29bad0b3903d77261e7706ff80fd292fefece3ebeaa4bb7f08a51ad2fc694";
    format!(
        r#"#!/bin/bash
set -euo pipefail
[ "$(uname -m)" = "x86_64" ] || {{ echo "only x86_64 supported by legacy installer" >&2; exit 1; }}
command -v curl >/dev/null 2>&1 || {{ apt-get update -qq && apt-get install -y -qq curl ca-certificates; }}
mkdir -p /opt/gizzi/bin
if [ ! -f /opt/gizzi/bin/gizzi-code ]; then
    curl -fsSL "https://github.com/Allternit/gizzi-code/releases/download/{release}/gizzi-code-linux-x64-native" -o /tmp/gizzi-code.download
    echo "{sha256}  /tmp/gizzi-code.download" | sha256sum -c -
    mv /tmp/gizzi-code.download /opt/gizzi/bin/gizzi-code
    chmod +x /opt/gizzi/bin/gizzi-code
fi
cat > /etc/systemd/system/gizzi-code.service << 'UNIT'
[Unit]
Description=gizzi-code instance
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=GIZZI_REQUIRE_CLERK_AUTH=true
Environment=GIZZI_DISABLE_AUTOUPDATE=1
ExecStart=/opt/gizzi/bin/gizzi-code serve --hostname 0.0.0.0 --port 4096
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable gizzi-code.service >/dev/null 2>&1 || true
systemctl restart gizzi-code.service
sleep 3
systemctl is-active --quiet gizzi-code.service
"#,
        release = RELEASE,
        sha256 = X64_SHA256,
    )
}
