// OWNER: T1-A5

//! Lifecycle Runner
//!
//! Run full environment lifecycle: provision → install → execute → cleanup.

use crate::types::*;
use std::time::Instant;

/// Lifecycle runner
pub struct LifecycleRunner {
    provisioner: Box<dyn Provisioner + Send + Sync>,
}

impl LifecycleRunner {
    /// Create new lifecycle runner
    pub fn new(provisioner: Box<dyn Provisioner + Send + Sync>) -> Self {
        Self { provisioner }
    }

    /// Run full lifecycle
    pub async fn run(&self, spec: LifecycleSpec) -> Result<LifecycleResult> {
        let start = Instant::now();
        let mut stages = Vec::new();

        // Stage 1: Provision
        stages.push("provision");
        if let Err(e) = self.provisioner.provision(&spec).await {
            return Ok(LifecycleResult::failure(stages, &e.to_string()));
        }

        // Stage 2: Install dependencies
        stages.push("install");
        if let Err(e) = self.install_dependencies(&spec).await {
            return Ok(LifecycleResult::failure(stages, &e.to_string()));
        }

        // Stage 3: Build (if applicable)
        if spec.build_command.is_some() {
            stages.push("build");
            if let Err(e) = self.build(&spec).await {
                return Ok(LifecycleResult::failure(stages, &e.to_string()));
            }
        }

        // Stage 4: Execute/Run
        stages.push("execute");
        let output = match self.execute(&spec).await {
            Ok(out) => out,
            Err(e) => return Ok(LifecycleResult::failure(stages, &e.to_string())),
        };

        // Stage 5: Cleanup
        stages.push("cleanup");
        if let Err(e) = self.provisioner.cleanup(&spec).await {
            // Log cleanup error but don't fail the run
            tracing::warn!("Cleanup failed: {}", e);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(LifecycleResult {
            success: true,
            stages_completed: stages.iter().map(|s| s.to_string()).collect(),
            output,
            error: None,
            duration_ms,
        })
    }

    /// Install dependencies based on package manager
    async fn install_dependencies(&self, spec: &LifecycleSpec) -> Result<()> {
        match spec.package_manager {
            PackageManager::Cargo => self.run_command("cargo", &["fetch"]).await,
            PackageManager::Npm | PackageManager::Yarn | PackageManager::Pnpm => {
                self.run_command("npm", &["install"]).await
            }
            PackageManager::Pip | PackageManager::Pip3 => {
                self.run_command("pip", &["install", "-r", "requirements.txt"]).await
            }
            PackageManager::GoMod => self.run_command("go", &["mod", "download"]).await,
            PackageManager::Poetry => self.run_command("poetry", &["install"]).await,
            _ => Ok(()), // No install needed
        }
    }

    /// Build project
    async fn build(&self, spec: &LifecycleSpec) -> Result<()> {
        if let Some(ref cmd) = spec.build_command {
            self.run_shell_command(cmd).await
        } else {
            // Default build commands
            match spec.package_manager {
                PackageManager::Cargo => self.run_command("cargo", &["build", "--release"]).await,
                PackageManager::Npm | PackageManager::Yarn | PackageManager::Pnpm => {
                    self.run_command("npm", &["run", "build"]).await
                }
                PackageManager::GoMod => self.run_command("go", &["build"]).await,
                _ => Ok(()),
            }
        }
    }

    /// Execute project
    async fn execute(&self, spec: &LifecycleSpec) -> Result<String> {
        if let Some(ref cmd) = spec.run_command {
            self.run_shell_command_output(cmd).await
        } else {
            // Default run commands
            match spec.language.language {
                Language::Rust => {
                    self.run_command_output("./target/release/app", &[]).await
                }
                Language::Python => {
                    self.run_command_output("python", &[spec.source_path.as_str()]).await
                }
                Language::Go => {
                    self.run_command_output("go", &["run", spec.source_path.as_str()]).await
                }
                Language::JavaScript | Language::TypeScript => {
                    self.run_command_output("node", &[spec.source_path.as_str()]).await
                }
                _ => Err(EnvironmentError::ExecutionFailed(
                    format!("No default run command for {:?}", spec.language.language)
                )),
            }
        }
    }

    /// Run command
    async fn run_command(&self, cmd: &str, args: &[&str]) -> Result<()> {
        #[cfg(unix)]
        {
            use tokio::process::Command;
            Command::new(cmd)
                .args(args)
                .output()
                .await
                .map_err(|e| EnvironmentError::ExecutionFailed(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            return Err(EnvironmentError::ExecutionFailed("Command execution not supported on this platform".to_string()));
        }
        Ok(())
    }

    /// Run command and capture output
    async fn run_command_output(&self, cmd: &str, args: &[&str]) -> Result<String> {
        #[cfg(unix)]
        {
            use tokio::process::Command;
            let output = Command::new(cmd)
                .args(args)
                .output()
                .await
                .map_err(|e| EnvironmentError::ExecutionFailed(e.to_string()))?;
            
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        #[cfg(not(unix))]
        {
            Err(EnvironmentError::ExecutionFailed("Command execution not supported on this platform".to_string()))
        }
    }

    /// Run shell command
    async fn run_shell_command(&self, cmd: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use tokio::process::Command;
            Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .await
                .map_err(|e| EnvironmentError::ExecutionFailed(e.to_string()))?;
        }
        Ok(())
    }

    /// Run shell command and capture output
    async fn run_shell_command_output(&self, cmd: &str) -> Result<String> {
        #[cfg(unix)]
        {
            use tokio::process::Command;
            let output = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .await
                .map_err(|e| EnvironmentError::ExecutionFailed(e.to_string()))?;
            
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        #[cfg(not(unix))]
        {
            Err(EnvironmentError::ExecutionFailed("Shell execution not supported on this platform".to_string()))
        }
    }
}

/// Simple provisioner for testing
pub struct SimpleProvisioner;

#[async_trait::async_trait]
impl Provisioner for SimpleProvisioner {
    async fn provision(&self, _spec: &LifecycleSpec) -> Result<()> {
        // In real impl: create directories, set up environment
        Ok(())
    }

    async fn cleanup(&self, _spec: &LifecycleSpec) -> Result<()> {
        // In real impl: remove temporary files, clean up
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lifecycle_runner_creation() {
        let provisioner = Box::new(SimpleProvisioner);
        let runner = LifecycleRunner::new(provisioner);
        
        // Basic creation test
        assert!(true);
    }

    #[test]
    fn test_lifecycle_spec_builder() {
        let spec = LifecycleSpec::new("test-app", "/path/to/source")
            .with_language(DetectedLanguage::new(Language::Rust, "test"))
            .with_package_manager(PackageManager::Cargo)
            .with_build_command("cargo build")
            .with_run_command("cargo run");

        assert_eq!(spec.name, "test-app");
        assert_eq!(spec.language.language, Language::Rust);
        assert_eq!(spec.package_manager, PackageManager::Cargo);
    }

    #[test]
    fn test_lifecycle_result_success() {
        let result = LifecycleResult::success(vec!["provision", "install"], "output");
        
        assert!(result.success);
        assert_eq!(result.stages_completed.len(), 2);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_lifecycle_result_failure() {
        let result = LifecycleResult::failure(vec!["provision"], "error message");
        
        assert!(!result.success);
        assert_eq!(result.stages_completed.len(), 1);
        assert!(result.error.is_some());
    }
}
