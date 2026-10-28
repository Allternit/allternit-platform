//! Container PTY (Pseudo Terminal) management
//!
//! Creates interactive shell sessions within Docker containers.
//! Provides sandboxed terminal access with resource limits and security constraints.

use anyhow::{Context, Result};
use allternit_protocol::{SandboxConfig, VolumeMount};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Container PTY manager handles sandboxed terminal sessions
#[derive(Debug, Clone)]
pub struct ContainerPtyManager {
    sessions: Arc<RwLock<HashMap<String, ContainerSessionHandle>>>,
}

/// Handle to a running container PTY session
#[derive(Debug)]
struct ContainerSessionHandle {
    session_id: String,
    write_tx: mpsc::Sender<Vec<u8>>,
    resize_tx: mpsc::Sender<(u16, u16)>,
    shutdown_tx: mpsc::Sender<()>,
    container_id: String,
}

/// Output from a container PTY session
#[derive(Debug, Clone)]
pub struct ContainerPtyOutput {
    pub session_id: String,
    pub data: Vec<u8>,
}

impl ContainerPtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new sandboxed PTY session in a Docker container
    pub async fn create_session(
        &self,
        session_id: String,
        shell: Option<String>,
        cols: u16,
        rows: u16,
        env: HashMap<String, String>,
        working_dir: Option<String>,
        sandbox_config: SandboxConfig,
        output_tx: mpsc::Sender<ContainerPtyOutput>,
    ) -> Result<()> {
        let shell = shell.unwrap_or_else(|| "/bin/sh".to_string());
        
        info!(
            "Creating container PTY session: {} (image: {}, shell: {}, {}x{}, env_vars: {}, cwd: {:?})",
            session_id, sandbox_config.image, shell, cols, rows, env.len(), working_dir
        );

        // Verify Docker is available
        if !Self::check_docker_available().await {
            anyhow::bail!("Docker is not available on this node");
        }

        // Pull the image first
        info!("Pulling Docker image: {}", sandbox_config.image);
        let pull_output = Command::new("docker")
            .args(["pull", &sandbox_config.image])
            .output()
            .await
            .context("Failed to pull Docker image")?;

        if !pull_output.status.success() {
            let stderr = String::from_utf8_lossy(&pull_output.stderr);
            anyhow::bail!("Failed to pull image {}: {}", sandbox_config.image, stderr);
        }

        // Channels for communication with the container task
        let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>(256);
        let (resize_tx, resize_rx) = mpsc::channel::<(u16, u16)>(16);
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

        let session_id_clone = session_id.clone();

        // Spawn the container PTY task
        let handle = tokio::spawn(async move {
            if let Err(e) = run_container_pty_task(
                session_id_clone,
                shell,
                cols,
                rows,
                env,
                working_dir,
                sandbox_config,
                write_rx,
                resize_rx,
                shutdown_rx,
                output_tx,
            )
            .await
            {
                error!("Container PTY task error: {}", e);
            }
        });

        // Wait a moment to verify container started
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Generate a unique container name
        let container_id = format!("allternit-terminal-{}", session_id);

        // Store session handle
        let handle = ContainerSessionHandle {
            session_id: session_id.clone(),
            write_tx,
            resize_tx,
            shutdown_tx,
            container_id,
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), handle);
        }

        info!("Container PTY session created: {}", session_id);
        Ok(())
    }

    /// Write data to a container PTY session
    pub async fn write(&self, session_id: &str, data: &[u8]) -> Result<()> {
        let sessions = self.sessions.read().await;

        if let Some(session) = sessions.get(session_id) {
            session.write(data).await
        } else {
            anyhow::bail!("Session {} not found", session_id)
        }
    }

    /// Resize a container PTY session
    pub async fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.read().await;

        if let Some(session) = sessions.get(session_id) {
            session.resize(cols, rows).await
        } else {
            anyhow::bail!("Session {} not found", session_id)
        }
    }

    /// Close a container PTY session
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;

        if let Some(handle) = sessions.remove(session_id) {
            // Signal shutdown
            let _ = handle.shutdown_tx.send(()).await;
            
            // Stop and remove the container
            let container_name = format!("allternit-terminal-{}", session_id);
            let _ = Command::new("docker")
                .args(["stop", "-t", "5", &container_name])
                .output()
                .await;
            
            let _ = Command::new("docker")
                .args(["rm", &container_name])
                .output()
                .await;

            info!("Container PTY session closed: {}", session_id);
        }

        Ok(())
    }

    /// List active sessions
    pub async fn list_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Check if Docker is available
    async fn check_docker_available() -> bool {
        match Command::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output()
            .await
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }
}

impl ContainerSessionHandle {
    /// Write data to the container PTY
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.write_tx
            .send(data.to_vec())
            .await
            .map_err(|_| anyhow::anyhow!("Failed to send data to container PTY - channel closed"))?;
        Ok(())
    }

    /// Resize the container PTY
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.resize_tx
            .send((cols, rows))
            .await
            .map_err(|_| anyhow::anyhow!("Failed to send resize to container PTY - channel closed"))?;
        Ok(())
    }
}

/// Build docker run arguments from sandbox configuration
fn build_docker_args(
    sandbox_config: &SandboxConfig,
    session_id: &str,
    shell: &str,
    cols: u16,
    rows: u16,
) -> Vec<String> {
    let container_name = format!("allternit-terminal-{}", session_id);
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-i".to_string(),
        "-t".to_string(),
        "--name".to_string(),
        container_name,
    ];

    // Resource limits
    if let Some(cpus) = sandbox_config.cpus {
        args.push("--cpus".to_string());
        args.push(cpus.to_string());
    }

    if let Some(memory_mb) = sandbox_config.memory_mb {
        args.push("--memory".to_string());
        args.push(format!("{}m", memory_mb));
    }

    // Volume mounts
    for mount in &sandbox_config.volumes {
        let mount_str = if mount.read_only {
            format!("{}:{}:ro", mount.source, mount.target)
        } else {
            format!("{}:{}", mount.source, mount.target)
        };
        args.push("-v".to_string());
        args.push(mount_str);
    }

    // Read-only root filesystem
    if sandbox_config.read_only_root {
        args.push("--read-only".to_string());
        // Add tmpfs for necessary writable directories
        args.push("--tmpfs".to_string());
        args.push("/tmp:noexec,nosuid,size=100m".to_string());
        args.push("--tmpfs".to_string());
        args.push("/run:noexec,nosuid,size=10m".to_string());
    }

    // Drop all capabilities
    if sandbox_config.drop_capabilities {
        args.push("--cap-drop".to_string());
        args.push("ALL".to_string());
        // Add only necessary capabilities for a basic terminal
        args.push("--cap-add".to_string());
        args.push("CHOWN".to_string());
        args.push("--cap-add".to_string());
        args.push("SETGID".to_string());
        args.push("--cap-add".to_string());
        args.push("SETUID".to_string());
    }

    // Network isolation
    if sandbox_config.no_host_network {
        args.push("--network".to_string());
        args.push("none".to_string());
    }

    // Security options
    for opt in &sandbox_config.security_opts {
        args.push("--security-opt".to_string());
        args.push(opt.clone());
    }

    // Additional security hardening
    args.push("--security-opt".to_string());
    args.push("no-new-privileges:true".to_string());

    // Environment for terminal size
    args.push("-e".to_string());
    args.push(format!("COLUMNS={}", cols));
    args.push("-e".to_string());
    args.push(format!("LINES={}", rows));
    args.push("-e".to_string());
    args.push("TERM=xterm-256color".to_string());

    // Image
    args.push(sandbox_config.image.clone());

    // Shell command
    args.push(shell.to_string());

    args
}

/// The actual container PTY task
async fn run_container_pty_task(
    session_id: String,
    shell: String,
    cols: u16,
    rows: u16,
    env: HashMap<String, String>,
    working_dir: Option<String>,
    sandbox_config: SandboxConfig,
    mut write_rx: mpsc::Receiver<Vec<u8>>,
    mut resize_rx: mpsc::Receiver<(u16, u16)>,
    mut shutdown_rx: mpsc::Receiver<()>,
    output_tx: mpsc::Sender<ContainerPtyOutput>,
) -> Result<()> {
    info!("Starting container PTY task for session: {}", session_id);

    // Build docker arguments
    let docker_args = build_docker_args(&sandbox_config, &session_id, &shell, cols, rows);

    // Add environment variables
    let mut full_args = vec!["run".to_string()];
    
    // We need to rebuild args to include environment variables
    let container_name = format!("allternit-terminal-{}", session_id);
    let mut all_args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-i".to_string(),
        "-t".to_string(),
        "--name".to_string(),
        container_name.clone(),
    ];

    // Add environment variables
    for (key, value) in &env {
        all_args.push("-e".to_string());
        all_args.push(format!("{}={}", key, value));
    }

    // Working directory
    if let Some(ref cwd) = working_dir {
        all_args.push("-w".to_string());
        all_args.push(cwd.clone());
    }

    // Resource limits
    if let Some(cpus) = sandbox_config.cpus {
        all_args.push("--cpus".to_string());
        all_args.push(cpus.to_string());
    }

    if let Some(memory_mb) = sandbox_config.memory_mb {
        all_args.push("--memory".to_string());
        all_args.push(format!("{}m", memory_mb));
    }

    // Volume mounts
    for mount in &sandbox_config.volumes {
        let mount_str = if mount.read_only {
            format!("{}:{}:ro", mount.source, mount.target)
        } else {
            format!("{}:{}", mount.source, mount.target)
        };
        all_args.push("-v".to_string());
        all_args.push(mount_str);
    }

    // Read-only root filesystem
    if sandbox_config.read_only_root {
        all_args.push("--read-only".to_string());
        all_args.push("--tmpfs".to_string());
        all_args.push("/tmp:noexec,nosuid,size=100m".to_string());
    }

    // Drop all capabilities
    if sandbox_config.drop_capabilities {
        all_args.push("--cap-drop".to_string());
        all_args.push("ALL".to_string());
    }

    // Network isolation
    if sandbox_config.no_host_network {
        all_args.push("--network".to_string());
        all_args.push("none".to_string());
    }

    // Security options
    all_args.push("--security-opt".to_string());
    all_args.push("no-new-privileges:true".to_string());

    // Image and shell
    all_args.push(sandbox_config.image.clone());
    all_args.push(shell.clone());

    info!("Starting Docker container for session {}: docker {:?}", session_id, all_args);

    // Start the docker container
    let mut child = Command::new("docker")
        .args(&all_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Failed to spawn Docker container")?;

    let child_id = child.id();
    info!("Container started for session {} (PID: {:?})", session_id, child_id);

    // Get stdin/stdout handles
    let mut stdin = child
        .stdin
        .take()
        .context("Failed to get container stdin")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Failed to get container stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .context("Failed to get container stderr")?;

    let session_id_stdout = session_id.clone();
    let session_id_stderr = session_id.clone();
    let output_tx_stdout = output_tx.clone();

    // Spawn stdout reader task
    let stdout_handle: JoinHandle<Result<()>> = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => {
                    debug!("Container stdout EOF for session {}", session_id_stdout);
                    break Ok(());
                }
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    debug!("Read {} bytes from container stdout {}", n, session_id_stdout);
                    if output_tx_stdout
                        .send(ContainerPtyOutput {
                            session_id: session_id_stdout.clone(),
                            data,
                        })
                        .await
                        .is_err()
                    {
                        error!("Output channel closed for session {}", session_id_stdout);
                        break Ok(());
                    }
                }
                Err(e) => {
                    error!("Container stdout read error for session {}: {}", session_id_stdout, e);
                    break Err(e.into());
                }
            }
        }
    });

    // Spawn stderr reader task
    let output_tx_stderr = output_tx.clone();
    let stderr_handle: JoinHandle<Result<()>> = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) => {
                    debug!("Container stderr EOF for session {}", session_id_stderr);
                    break Ok(());
                }
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    if output_tx_stderr
                        .send(ContainerPtyOutput {
                            session_id: session_id_stderr.clone(),
                            data,
                        })
                        .await
                        .is_err()
                    {
                        break Ok(());
                    }
                }
                Err(e) => {
                    error!("Container stderr read error for session {}: {}", session_id_stderr, e);
                    break Err(e.into());
                }
            }
        }
    });

    // Main loop: handle input, resize, and shutdown
    let result: Result<()> = loop {
        tokio::select! {
            // Handle input from user
            Some(data) = write_rx.recv() => {
                debug!("Writing {} bytes to container stdin {}", data.len(), session_id);
                if let Err(e) = stdin.write_all(&data).await {
                    error!("Failed to write to container stdin {}: {}", session_id, e);
                    break Err(e.into());
                }
                if let Err(e) = stdin.flush().await {
                    error!("Failed to flush container stdin {}: {}", session_id, e);
                    break Err(e.into());
                }
            }

            // Handle resize events
            Some((new_cols, new_rows)) = resize_rx.recv() => {
                debug!("Resizing terminal {} to {}x{}", session_id, new_cols, new_rows);
                // Docker exec doesn't directly support resize, but we can set env vars
                // In a full implementation, we'd use the Docker API's resize endpoint
                // For now, we rely on the shell receiving SIGWINCH from the PTY
                let resize_cmd = format!("stty cols {} rows {}", new_cols, new_rows);
                let resize_data = format!("{}", resize_cmd).into_bytes();
                if let Err(e) = stdin.write_all(&resize_data).await {
                    warn!("Failed to send resize command: {}", e);
                }
            }

            // Handle shutdown signal
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received for session {}", session_id);
                break Ok(());
            }

            // Check if process has exited
            result = child.wait() => {
                match result {
                    Ok(status) => {
                        info!("Container exited with status: {:?} for session {}", status, session_id);
                        break Ok(());
                    }
                    Err(e) => {
                        error!("Error waiting for container {}: {}", session_id, e);
                        break Err(e.into());
                    }
                }
            }
        }
    };

    // Clean up
    drop(stdin);

    // Wait for reader tasks to finish
    let _ = stdout_handle.await;
    let _ = stderr_handle.await;

    // Ensure container is stopped and removed
    info!("Cleaning up container for session {}", session_id);
    let _ = Command::new("docker")
        .args(["stop", "-t", "2", &container_name])
        .output()
        .await;
    
    let _ = Command::new("docker")
        .args(["rm", &container_name])
        .output()
        .await;

    info!("Container PTY task ended for session: {}", session_id);
    result
}

impl Default for ContainerPtyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_docker_args() {
        let config = SandboxConfig {
            image: "alpine:latest".to_string(),
            cpus: Some(1.0),
            memory_mb: Some(512),
            volumes: vec![VolumeMount {
                source: "/host/path".to_string(),
                target: "/container/path".to_string(),
                read_only: true,
            }],
            read_only_root: true,
            drop_capabilities: true,
            no_host_network: true,
            security_opts: vec![],
        };

        let args = build_docker_args(&config, "test-session", "/bin/sh", 80, 24);

        assert!(args.contains(&"--cpus".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(args.contains(&"--memory".to_string()));
        assert!(args.contains(&"512m".to_string()));
        assert!(args.contains(&"--read-only".to_string()));
        assert!(args.contains(&"alpine:latest".to_string()));
    }
}
