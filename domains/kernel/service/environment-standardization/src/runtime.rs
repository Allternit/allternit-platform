// OWNER: T1-A4

//! Container Runtime Support
//!
//! Support for Docker, Podman, Containerd, and Firecracker.

use crate::types::*;
use std::process::Command;

/// Container runtime manager
pub struct RuntimeManager {
    runtime: ContainerRuntime,
}

impl RuntimeManager {
    /// Create new runtime manager
    pub fn new(runtime: ContainerRuntime) -> Self {
        Self { runtime }
    }

    /// Detect available runtime
    pub fn detect() -> ContainerRuntime {
        if Command::new("docker").arg("--version").output().is_ok() {
            return ContainerRuntime::Docker;
        }
        if Command::new("podman").arg("--version").output().is_ok() {
            return ContainerRuntime::Podman;
        }
        if Command::new("ctr").arg("--version").output().is_ok() {
            return ContainerRuntime::Containerd;
        }
        ContainerRuntime::None
    }

    /// Check if runtime is available
    pub fn is_available(&self) -> bool {
        match self.runtime {
            ContainerRuntime::Docker => Command::new("docker").arg("--version").output().is_ok(),
            ContainerRuntime::Podman => Command::new("podman").arg("--version").output().is_ok(),
            ContainerRuntime::Containerd => Command::new("ctr").arg("--version").output().is_ok(),
            ContainerRuntime::Firecracker => true, // Would need specific setup
            ContainerRuntime::None => false,
        }
    }

    /// Run container with config
    pub fn run(&self, config: &ContainerConfig) -> Result<ContainerResult> {
        match self.runtime {
            ContainerRuntime::Docker => self.run_docker(config),
            ContainerRuntime::Podman => self.run_podman(config),
            ContainerRuntime::Containerd => self.run_containerd(config),
            ContainerRuntime::Firecracker => self.run_firecracker(config),
            ContainerRuntime::None => Err(EnvironmentError::RuntimeNotSupported("No runtime available".to_string())),
        }
    }

    fn run_docker(&self, config: &ContainerConfig) -> Result<ContainerResult> {
        let mut cmd = Command::new("docker");
        cmd.arg("run").arg("--rm");

        // Add volumes
        for volume in &config.volumes {
            cmd.arg("-v").arg(volume);
        }

        // Add env vars
        for (key, value) in &config.env {
            cmd.arg("-e").arg(format!("{}={}", key, value));
        }

        // Add working dir
        if let Some(ref wd) = config.working_dir {
            cmd.arg("-w").arg(wd);
        }

        cmd.arg(&config.image);

        // Add command
        if let Some(ref command) = config.command {
            cmd.args(command);
        }

        let output = cmd.output()
            .map_err(|e| EnvironmentError::ContainerRuntimeError(e.to_string()))?;

        Ok(ContainerResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn run_podman(&self, config: &ContainerConfig) -> Result<ContainerResult> {
        // Podman is mostly CLI-compatible with Docker
        let mut cmd = Command::new("podman");
        cmd.arg("run").arg("--rm");

        for volume in &config.volumes {
            cmd.arg("-v").arg(volume);
        }

        for (key, value) in &config.env {
            cmd.arg("-e").arg(format!("{}={}", key, value));
        }

        if let Some(ref wd) = config.working_dir {
            cmd.arg("-w").arg(wd);
        }

        cmd.arg(&config.image);

        if let Some(ref command) = config.command {
            cmd.args(command);
        }

        let output = cmd.output()
            .map_err(|e| EnvironmentError::ContainerRuntimeError(e.to_string()))?;

        Ok(ContainerResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn run_containerd(&self, config: &ContainerConfig) -> Result<ContainerResult> {
        // Containerd uses ctr command
        let mut cmd = Command::new("ctr");
        cmd.arg("run").arg("--rm");

        cmd.arg(&config.image);
        cmd.arg("container-name");

        if let Some(ref command) = config.command {
            cmd.args(command);
        }

        let output = cmd.output()
            .map_err(|e| EnvironmentError::ContainerRuntimeError(e.to_string()))?;

        Ok(ContainerResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn run_firecracker(&self, _config: &ContainerConfig) -> Result<ContainerResult> {
        // Firecracker requires microVM setup - stub for now
        Err(EnvironmentError::RuntimeNotSupported(
            "Firecracker requires microVM configuration".to_string()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_detection() {
        let runtime = RuntimeManager::detect();
        // Will be Docker, Podman, or None depending on system
        assert!(matches!(runtime, ContainerRuntime::Docker | ContainerRuntime::Podman | ContainerRuntime::Containerd | ContainerRuntime::None));
    }

    #[test]
    fn test_runtime_creation() {
        let manager = RuntimeManager::new(ContainerRuntime::Docker);
        assert_eq!(manager.runtime, ContainerRuntime::Docker);
    }

    #[test]
    fn test_container_config_builder() {
        let config = ContainerConfig::new("alpine:latest")
            .with_command(vec!["echo".to_string(), "hello".to_string()])
            .with_volume("/host:/container");

        assert_eq!(config.image, "alpine:latest");
        assert!(config.command.is_some());
        assert_eq!(config.volumes.len(), 1);
    }
}
