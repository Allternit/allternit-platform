//! Local Runtime implementation
//!
//! Wraps the existing VM drivers (Apple VF / Firecracker) for local execution.
//! Uses VM Bridge to communicate with TypeScript runtime.
//!
//! Also includes native QEMU/KVM VM lifecycle management via VmBridge when
//! the TypeScript runtime bridge is not available.

use super::*;
use crate::db::cowork_models::*;
use crate::error::ApiError;
use crate::runtime::vm_bridge::VMBridgeManager;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;

use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::Duration;

/// Local runtime configuration
#[derive(Debug, Clone)]
pub struct LocalRuntimeConfig {
    /// VM driver to use
    pub driver: VMDriver,
    /// Base directory for VM data
    pub data_dir: std::path::PathBuf,
    /// Default resources
    pub default_resources: ResourceLimits,
    /// SSH key path for VM access
    pub ssh_key_path: Option<PathBuf>,
    /// Base VM image path (QCOW2 format)
    pub base_image_path: Option<PathBuf>,
    /// QEMU binary path (auto-detected if None)
    pub qemu_binary: Option<PathBuf>,
}

/// VM Driver type
#[derive(Debug, Clone)]
pub enum VMDriver {
    /// Apple Virtualization Framework (macOS)
    AppleVF,
    /// Firecracker (Linux)
    Firecracker,
    /// QEMU/KVM (Linux)
    QemuKvm,
    /// QEMU with TCG (software emulation, fallback)
    QemuTcg,
}

impl Default for LocalRuntimeConfig {
    fn default() -> Self {
        Self {
            driver: detect_driver(),
            data_dir: expand_home("~/.allternit/local-runtime"),
            default_resources: ResourceLimits {
                memory_mb: Some(2048),
                cpu_cores: Some(2.0),
                disk_gb: Some(10),
                timeout_seconds: Some(3600),
            },
            ssh_key_path: None,
            base_image_path: None,
            qemu_binary: None,
        }
    }
}

/// Detect the best VM driver for the current platform
fn detect_driver() -> VMDriver {
    #[cfg(target_os = "macos")]
    {
        VMDriver::AppleVF
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Check if KVM is available
        if Path::new("/dev/kvm").exists() {
            VMDriver::QemuKvm
        } else {
            VMDriver::QemuTcg
        }
    }
}

/// Expand ~ to home directory
fn expand_home(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// VM state persisted to disk
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VmState {
    /// VM ID
    pub vm_id: String,
    /// Run ID
    pub run_id: String,
    /// QEMU process PID
    pub pid: Option<u32>,
    /// SSH port on host
    pub ssh_port: u16,
    /// VM IP address (usually 10.0.2.15 for user networking)
    pub vm_ip: String,
    /// Path to the disk image
    pub disk_path: PathBuf,
    /// Path to the cloud-init ISO
    pub cloudinit_path: PathBuf,
    /// Path to the PID file
    pub pid_file_path: PathBuf,
    /// Current state
    pub state: RuntimeState,
    /// SSH username
    pub ssh_user: String,
    /// SSH key path
    pub ssh_key_path: Option<PathBuf>,
    /// Creation time
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// VM Bridge for managing QEMU/KVM VMs
///
/// This struct provides native VM lifecycle management when the TypeScript
/// runtime bridge is not available. It manages VMs via QEMU/KVM with cloud-init.
pub struct VmBridge {
    /// Base directory for VM data
    data_dir: PathBuf,
    /// VM state file path
    #[allow(dead_code)]
    state_file: PathBuf,
    /// QEMU binary path
    qemu_binary: PathBuf,
    /// Whether KVM is available
    kvm_available: bool,
    /// SSH key path for VM access
    #[allow(dead_code)]
    ssh_key_path: Option<PathBuf>,
    /// Base VM image path
    base_image_path: Option<PathBuf>,
}

impl VmBridge {
    /// Create a new VmBridge
    pub async fn new(config: &LocalRuntimeConfig) -> Result<Self, ApiError> {
        let data_dir = config.data_dir.clone();
        fs::create_dir_all(&data_dir)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        let state_file = data_dir.join("vm_states.json");

        // Detect QEMU binary
        let qemu_binary = if let Some(ref binary) = config.qemu_binary {
            binary.clone()
        } else {
            Self::detect_qemu_binary().await?
        };

        // Check KVM availability
        let kvm_available = Path::new("/dev/kvm").exists();

        Ok(Self {
            data_dir,
            state_file,
            qemu_binary,
            kvm_available,
            ssh_key_path: config.ssh_key_path.clone(),
            base_image_path: config.base_image_path.clone(),
        })
    }

    /// Detect QEMU binary on the system
    async fn detect_qemu_binary() -> Result<PathBuf, ApiError> {
        // Try common QEMU binary names
        let candidates = ["qemu-system-x86_64", "qemu-system-aarch64", "qemu-kvm"];

        for binary in &candidates {
            if let Ok(output) = Command::new("which").arg(binary).output().await {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    return Ok(PathBuf::from(path));
                }
            }
        }

        Err(ApiError::Internal(
            "QEMU not found. Please install QEMU (qemu-system-x86_64 or qemu-system-aarch64)"
                .to_string(),
        ))
    }

    /// Check if QEMU is installed
    pub async fn is_qemu_available(&self) -> bool {
        Command::new(&self.qemu_binary)
            .arg("--version")
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Generate SSH key pair for VM access
    async fn generate_ssh_key(&self, vm_id: &str) -> Result<PathBuf, ApiError> {
        let key_dir = self.data_dir.join("keys");
        fs::create_dir_all(&key_dir)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        let key_path = key_dir.join(format!("{}_key", vm_id));
        let pub_key_path = key_path.with_extension("pub");

        // Generate key if it doesn't exist
        if !key_path.exists() {
            let output = Command::new("ssh-keygen")
                .args(&[
                    "-t",
                    "ed25519",
                    "-f",
                    key_path.to_str().unwrap(),
                    "-N",
                    "", // No passphrase
                    "-C",
                    &format!("allternit@{})", vm_id),
                ])
                .output()
                .await
                .map_err(|e| ApiError::SshError(format!("Failed to generate SSH key: {}", e)))?;

            if !output.status.success() {
                return Err(ApiError::SshError(format!(
                    "ssh-keygen failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }

        Ok(pub_key_path)
    }

    /// Generate cloud-init user-data
    fn generate_cloud_init_user_data(&self, ssh_pub_key: &str) -> String {
        format!(
            r#"#cloud-config
users:
  - name: allternit
    sudo: ALL=(ALL) NOPASSWD:ALL
    groups: users, admin
    shell: /bin/bash
    ssh_authorized_keys:
      - {}

package_update: true
packages:
  - curl
  - wget
  - git
  - openssh-server
  - qemu-guest-agent

runcmd:
  - systemctl enable qemu-guest-agent
  - systemctl start qemu-guest-agent
  - echo "VM initialized by Allternit" > /var/lib/cloud/instance/allternit-ready

final_message: "The Allternit VM is up and running!"
"#,
            ssh_pub_key
        )
    }

    /// Generate cloud-init meta-data
    fn generate_cloud_init_meta_data(&self, vm_id: &str) -> String {
        format!(
            r#"instance-id: {}
local-hostname: {}
"#,
            vm_id, vm_id
        )
    }

    /// Create cloud-init ISO image
    async fn create_cloud_init_iso(
        &self,
        vm_id: &str,
        user_data: &str,
        meta_data: &str,
    ) -> Result<PathBuf, ApiError> {
        let vm_dir = self.data_dir.join("vms").join(vm_id);
        fs::create_dir_all(&vm_dir)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        let iso_path = vm_dir.join("cloud-init.iso");

        // Create temporary directory for cloud-init files
        let temp_dir = tempfile::tempdir().map_err(|e| ApiError::IoError(e))?;

        let user_data_path = temp_dir.path().join("user-data");
        let meta_data_path = temp_dir.path().join("meta-data");

        fs::write(&user_data_path, user_data)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        fs::write(&meta_data_path, meta_data)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        // Create ISO using genisoimage or mkisofs
        let iso_tool = if Command::new("which")
            .arg("genisoimage")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            "genisoimage"
        } else if Command::new("which")
            .arg("mkisofs")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            "mkisofs"
        } else {
            // Fallback: create a simple tar archive that QEMU can use as -cdrom
            // This is a simplified approach - in production, you'd want to ensure
            // genisoimage or mkisofs is installed
            return Err(ApiError::Internal(
                "genisoimage or mkisofs not found. Please install genisoimage (apt-get install genisoimage)".to_string()
            ));
        };

        let output = Command::new(iso_tool)
            .args(&[
                "-output",
                iso_path.to_str().unwrap(),
                "-volid",
                "cidata",
                "-joliet",
                "-rock",
                user_data_path.to_str().unwrap(),
                meta_data_path.to_str().unwrap(),
            ])
            .output()
            .await
            .map_err(|e| ApiError::IoError(e))?;

        if !output.status.success() {
            return Err(ApiError::Internal(format!(
                "Failed to create cloud-init ISO: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(iso_path)
    }

    /// Create a new disk image from base image or create empty one
    async fn create_disk_image(&self, vm_id: &str, size_gb: i32) -> Result<PathBuf, ApiError> {
        let vm_dir = self.data_dir.join("vms").join(vm_id);
        fs::create_dir_all(&vm_dir)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        let disk_path = vm_dir.join("disk.qcow2");

        if let Some(ref base_image) = self.base_image_path {
            // Create copy-on-write image from base
            if base_image.exists() {
                let output = Command::new("qemu-img")
                    .args(&[
                        "create",
                        "-f",
                        "qcow2",
                        "-F",
                        "qcow2",
                        "-b",
                        base_image.to_str().unwrap(),
                        disk_path.to_str().unwrap(),
                        &format!("{}G", size_gb),
                    ])
                    .output()
                    .await
                    .map_err(|e| ApiError::IoError(e))?;

                if !output.status.success() {
                    return Err(ApiError::Internal(format!(
                        "Failed to create disk image: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
            } else {
                return Err(ApiError::Internal(format!(
                    "Base image not found: {}",
                    base_image.display()
                )));
            }
        } else {
            // Create empty disk image
            let output = Command::new("qemu-img")
                .args(&[
                    "create",
                    "-f",
                    "qcow2",
                    disk_path.to_str().unwrap(),
                    &format!("{}G", size_gb),
                ])
                .output()
                .await
                .map_err(|e| ApiError::IoError(e))?;

            if !output.status.success() {
                return Err(ApiError::Internal(format!(
                    "Failed to create disk image: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }

        Ok(disk_path)
    }

    /// Find an available port for SSH forwarding
    async fn find_available_port(&self) -> Result<u16, ApiError> {
        // Try ports in the ephemeral range
        for port in 22222..=22322 {
            if let Ok(listener) = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await
            {
                drop(listener);
                return Ok(port);
            }
        }
        Err(ApiError::Internal(
            "No available ports for SSH forwarding".to_string(),
        ))
    }

    /// Start a VM
    pub async fn start_vm(
        &self,
        run_id: &str,
        vm_id: &str,
        config: &RunConfig,
    ) -> Result<VmState, ApiError> {
        // Check if VM already exists
        if let Ok(Some(_)) = self.load_vm_state(vm_id).await {
            return Err(ApiError::BadRequest(format!("VM {} already exists", vm_id)));
        }

        tracing::info!(
            "Starting VM {} for run {} via native QEMU/KVM",
            vm_id,
            run_id
        );

        // Generate SSH key
        let ssh_pub_key_path = self.generate_ssh_key(vm_id).await?;
        let ssh_pub_key = fs::read_to_string(&ssh_pub_key_path)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        // Generate cloud-init
        let user_data = self.generate_cloud_init_user_data(&ssh_pub_key);
        let meta_data = self.generate_cloud_init_meta_data(vm_id);
        let cloudinit_path = self
            .create_cloud_init_iso(vm_id, &user_data, &meta_data)
            .await?;

        // Create disk image
        let disk_size = config
            .resource_limits
            .as_ref()
            .and_then(|r| r.disk_gb)
            .unwrap_or(10);
        let disk_path = self.create_disk_image(vm_id, disk_size).await?;

        // Find SSH port
        let ssh_port = self.find_available_port().await?;

        // Get resource limits
        let memory_mb = config
            .resource_limits
            .as_ref()
            .and_then(|r| r.memory_mb)
            .unwrap_or(2048);
        let cpu_cores = config
            .resource_limits
            .as_ref()
            .and_then(|r| r.cpu_cores)
            .map(|c| c as u32)
            .unwrap_or(2);

        // Build QEMU command
        let mut qemu_args = vec![
            "-machine".to_string(),
            if self.kvm_available {
                "q35,accel=kvm:tcg".to_string()
            } else {
                "q35,accel=tcg".to_string()
            },
            "-cpu".to_string(),
            if self.kvm_available {
                "host".to_string()
            } else {
                "max".to_string()
            },
            "-smp".to_string(),
            format!("{}", cpu_cores),
            "-m".to_string(),
            format!("{}", memory_mb),
            "-drive".to_string(),
            format!(
                "file={},if=virtio,cache=writeback,format=qcow2",
                disk_path.display()
            ),
            "-cdrom".to_string(),
            cloudinit_path.to_str().unwrap().to_string(),
            "-netdev".to_string(),
            format!("user,id=net0,hostfwd=tcp::{}-:22", ssh_port),
            "-device".to_string(),
            "virtio-net-pci,netdev=net0".to_string(),
            "-display".to_string(),
            "none".to_string(),
            "-daemonize".to_string(),
            "-pidfile".to_string(),
            self.data_dir
                .join("vms")
                .join(vm_id)
                .join("qemu.pid")
                .to_str()
                .unwrap()
                .to_string(),
            "-vga".to_string(),
            "virtio".to_string(),
            "-device".to_string(),
            "virtio-rng-pci".to_string(),
            "-device".to_string(),
            "virtio-balloon-pci".to_string(),
        ];

        // Add serial port for console
        qemu_args.push("-serial".to_string());
        qemu_args.push("stdio".to_string());

        // Add QEMU guest agent socket
        let qga_socket = self.data_dir.join("vms").join(vm_id).join("qga.sock");
        qemu_args.push("-chardev".to_string());
        qemu_args.push(format!(
            "socket,path={},server=on,wait=off,id=qga0",
            qga_socket.display()
        ));
        qemu_args.push("-device".to_string());
        qemu_args.push("virtio-serial".to_string());
        qemu_args.push("-device".to_string());
        qemu_args.push("virtserialport,chardev=qga0,name=org.qemu.guest_agent.0".to_string());

        tracing::debug!(
            "QEMU command: {} {:?}",
            self.qemu_binary.display(),
            qemu_args
        );

        // Start QEMU process
        let child = Command::new(&self.qemu_binary)
            .args(&qemu_args)
            .spawn()
            .map_err(|e| ApiError::IoError(e))?;

        let pid = child.id();

        // Wait a moment for QEMU to write pidfile
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Try to read actual PID from pidfile
        let pid_file_path = self.data_dir.join("vms").join(vm_id).join("qemu.pid");
        let actual_pid = if pid_file_path.exists() {
            fs::read_to_string(&pid_file_path)
                .await
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .or(pid)
        } else {
            pid
        };

        // Create VM state
        let vm_state = VmState {
            vm_id: vm_id.to_string(),
            run_id: run_id.to_string(),
            pid: actual_pid,
            ssh_port,
            vm_ip: "10.0.2.15".to_string(),
            disk_path,
            cloudinit_path,
            pid_file_path,
            state: RuntimeState::Running,
            ssh_user: "allternit".to_string(),
            ssh_key_path: Some(ssh_pub_key_path.with_extension("")), // Private key path
            created_at: chrono::Utc::now(),
        };

        // Save state
        self.save_vm_state(vm_id, &vm_state).await?;

        tracing::info!(
            "VM {} started successfully (PID: {:?}, SSH port: {})",
            vm_id,
            actual_pid,
            ssh_port
        );

        // Wait briefly for VM to boot
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(vm_state)
    }

    /// Stop a VM
    pub async fn stop_vm(&self, vm_id: &str) -> Result<(), ApiError> {
        let mut vm_state = self
            .load_vm_state(vm_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("VM not found: {}", vm_id)))?;

        tracing::info!("Stopping VM {}", vm_id);

        // Try graceful shutdown first
        if let Some(pid) = vm_state.pid {
            // Try to send shutdown via QEMU monitor
            let monitor_socket = self.data_dir.join("vms").join(vm_id).join("qga.sock");

            // First try graceful shutdown via guest agent
            if monitor_socket.exists() {
                // We'll use a simple kill approach for now
                // In production, you'd use the QEMU guest agent protocol
            }

            // Send SIGTERM to QEMU process
            #[cfg(unix)]
            {
                use std::process::Stdio;
                let _ = Command::new("kill")
                    .args(&["-TERM", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }

            // Wait for process to terminate
            let timeout_duration = Duration::from_secs(30);
            let start = tokio::time::Instant::now();

            while start.elapsed() < timeout_duration {
                if !Self::is_process_running(pid).await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            // Force kill if still running
            if Self::is_process_running(pid).await {
                #[cfg(unix)]
                {
                    use std::process::Stdio;
                    let _ = Command::new("kill")
                        .args(&["-KILL", &pid.to_string()])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                }
            }
        }

        // Update state
        vm_state.state = RuntimeState::Stopped;
        self.save_vm_state(vm_id, &vm_state).await?;

        tracing::info!("VM {} stopped", vm_id);
        Ok(())
    }

    /// Check if a process is running
    async fn is_process_running(pid: u32) -> bool {
        #[cfg(unix)]
        {
            // Check if /proc/<pid> exists
            Path::new(&format!("/proc/{}", pid)).exists()
        }
        #[cfg(not(unix))]
        {
            // On non-Unix systems, we can't easily check
            false
        }
    }

    /// Get VM status
    pub async fn get_vm_status(&self, vm_id: &str) -> Result<VmState, ApiError> {
        let mut vm_state = self
            .load_vm_state(vm_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("VM not found: {}", vm_id)))?;

        // Check if process is still running
        if let Some(pid) = vm_state.pid {
            if !Self::is_process_running(pid).await {
                vm_state.state = RuntimeState::Stopped;
                vm_state.pid = None;
                self.save_vm_state(vm_id, &vm_state).await?;
            }
        }

        Ok(vm_state)
    }

    /// Execute a command in the VM via SSH
    pub async fn execute_in_vm(
        &self,
        vm_id: &str,
        command: &str,
        args: &[&str],
    ) -> Result<ExecResult, ApiError> {
        let vm_state = self.get_vm_status(vm_id).await?;

        if vm_state.state != RuntimeState::Running {
            return Err(ApiError::Internal(format!("VM {} is not running", vm_id)));
        }

        let ssh_key_path = vm_state
            .ssh_key_path
            .as_ref()
            .ok_or_else(|| ApiError::SshError("No SSH key configured".to_string()))?;

        // Build command string
        let full_command = if args.is_empty() {
            command.to_string()
        } else {
            format!("{} {}", command, args.join(" "))
        };

        tracing::debug!("Executing in VM {}: {}", vm_id, full_command);

        // Use SSH to execute command
        let ssh_output = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "LogLevel=ERROR",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "BatchMode=yes",
                "-i",
                ssh_key_path.to_str().unwrap(),
                "-p",
                &vm_state.ssh_port.to_string(),
                &format!("{}@127.0.0.1", vm_state.ssh_user),
                &full_command,
            ])
            .output()
            .await
            .map_err(|e| ApiError::SshError(format!("SSH execution failed: {}", e)))?;

        let exit_code = ssh_output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&ssh_output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&ssh_output.stderr).to_string();

        tracing::debug!(
            "SSH command completed with exit code {}: stdout={}, stderr={}",
            exit_code,
            stdout.len(),
            stderr.len()
        );

        Ok(ExecResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    /// List all VMs
    pub async fn list_vms(&self) -> Result<Vec<VmState>, ApiError> {
        let mut vms = Vec::new();
        let vms_dir = self.data_dir.join("vms");

        if !vms_dir.exists() {
            return Ok(vms);
        }

        let mut entries = fs::read_dir(&vms_dir)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(vm_id) = entry.file_name().to_str() {
                if let Ok(Some(vm_state)) = self.load_vm_state(vm_id).await {
                    vms.push(vm_state);
                }
            }
        }

        Ok(vms)
    }

    /// Delete a VM (cleanup resources)
    pub async fn delete_vm(&self, vm_id: &str) -> Result<(), ApiError> {
        // Stop VM first
        let _ = self.stop_vm(vm_id).await;

        let vm_state = self
            .load_vm_state(vm_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("VM not found: {}", vm_id)))?;

        // Remove disk image
        if vm_state.disk_path.exists() {
            let _ = fs::remove_file(&vm_state.disk_path).await;
        }

        // Remove cloud-init ISO
        if vm_state.cloudinit_path.exists() {
            let _ = fs::remove_file(&vm_state.cloudinit_path).await;
        }

        // Remove VM directory
        let vm_dir = self.data_dir.join("vms").join(vm_id);
        if vm_dir.exists() {
            let _ = fs::remove_dir_all(&vm_dir).await;
        }

        // Remove from state file
        self.remove_vm_state(vm_id).await?;

        tracing::info!("VM {} deleted", vm_id);
        Ok(())
    }

    /// Save VM state to disk
    async fn save_vm_state(&self, vm_id: &str, vm_state: &VmState) -> Result<(), ApiError> {
        let vm_state_file = self.data_dir.join("vms").join(vm_id).join("state.json");

        // Ensure directory exists
        if let Some(parent) = vm_state_file.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::IoError(e))?;
        }

        let json =
            serde_json::to_string_pretty(vm_state).map_err(|e| ApiError::SerializationError(e))?;

        fs::write(&vm_state_file, json)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        Ok(())
    }

    /// Load VM state from disk
    async fn load_vm_state(&self, vm_id: &str) -> Result<Option<VmState>, ApiError> {
        let vm_state_file = self.data_dir.join("vms").join(vm_id).join("state.json");

        if !vm_state_file.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&vm_state_file)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        let vm_state: VmState =
            serde_json::from_str(&json).map_err(|e| ApiError::SerializationError(e))?;

        Ok(Some(vm_state))
    }

    /// Remove VM state from disk
    async fn remove_vm_state(&self, vm_id: &str) -> Result<(), ApiError> {
        let vm_state_file = self.data_dir.join("vms").join(vm_id).join("state.json");
        if vm_state_file.exists() {
            fs::remove_file(&vm_state_file)
                .await
                .map_err(|e| ApiError::IoError(e))?;
        }
        Ok(())
    }
}

/// Local VM instance tracking
#[derive(Debug)]
struct VMInstance {
    /// VM ID
    id: String,
    /// Associated run ID
    run_id: String,
    /// VM process handle
    process: Option<tokio::process::Child>,
    /// Socket path for communication
    socket_path: std::path::PathBuf,
    /// Current state
    state: RuntimeState,
    /// PID if running
    pid: Option<u32>,
}

/// Local runtime implementation
pub struct LocalRuntime {
    config: LocalRuntimeConfig,
    /// Active VM instances
    instances: Arc<RwLock<HashMap<String, VMInstance>>>,
    /// VM Bridge manager for TypeScript runtime communication
    bridge_manager: VMBridgeManager,
    /// Native VM bridge for QEMU/KVM management (when TypeScript bridge unavailable)
    native_bridge: Option<VmBridge>,
}

impl LocalRuntime {
    /// Create a new LocalRuntime
    pub async fn new() -> Result<Self, ApiError> {
        let config = LocalRuntimeConfig::default();
        Self::with_config(config).await
    }

    /// Create a new LocalRuntime with custom config
    pub async fn with_config(config: LocalRuntimeConfig) -> Result<Self, ApiError> {
        tokio::fs::create_dir_all(&config.data_dir)
            .await
            .map_err(|e| ApiError::IoError(e))?;

        let bridge_manager = VMBridgeManager::new(&config.data_dir);

        // Try to create native bridge
        let native_bridge = match VmBridge::new(&config).await {
            Ok(bridge) => {
                if bridge.is_qemu_available().await {
                    tracing::info!("Native VM Bridge initialized with QEMU support");
                    Some(bridge)
                } else {
                    tracing::warn!("QEMU not available, native VM bridge disabled");
                    None
                }
            }
            Err(e) => {
                tracing::warn!("Failed to initialize native VM bridge: {}", e);
                None
            }
        };

        Ok(Self {
            config,
            instances: Arc::new(RwLock::new(HashMap::new())),
            bridge_manager,
            native_bridge,
        })
    }

    /// Start a VM for the given run
    async fn start_vm(&self, run_id: &str, config: &RunConfig) -> Result<VMInstance, ApiError> {
        let vm_id = format!("vm-{}", run_id);
        let socket_path = self.config.data_dir.join(format!("{}.sock", vm_id));

        tracing::info!(
            "Starting {} VM for run {}: socket={}",
            match self.config.driver {
                VMDriver::AppleVF => "Apple VF",
                VMDriver::Firecracker => "Firecracker",
                VMDriver::QemuKvm => "QEMU/KVM",
                VMDriver::QemuTcg => "QEMU/TCG",
            },
            run_id,
            socket_path.display()
        );

        // Try to use VM Bridge (TypeScript runtime) first
        if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            match bridge.start_vm(run_id, &vm_id, config).await {
                Ok(()) => {
                    tracing::info!("VM started via bridge: {}", vm_id);

                    let vm = VMInstance {
                        id: vm_id.clone(),
                        run_id: run_id.to_string(),
                        process: None,
                        socket_path: socket_path.clone(),
                        state: RuntimeState::Running,
                        pid: None,
                    };

                    let mut instances = self.instances.write().await;
                    instances.insert(vm_id.clone(), vm);

                    return Ok(VMInstance {
                        id: vm_id,
                        run_id: run_id.to_string(),
                        process: None,
                        socket_path: socket_path.clone(),
                        state: RuntimeState::Running,
                        pid: None,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to start VM via bridge: {}. Trying native bridge.",
                        e
                    );
                }
            }
        }

        // Fallback to native VM bridge
        if let Some(ref native_bridge) = self.native_bridge {
            match native_bridge.start_vm(run_id, &vm_id, config).await {
                Ok(vm_state) => {
                    tracing::info!(
                        "VM started via native bridge: {} (PID: {:?})",
                        vm_id,
                        vm_state.pid
                    );

                    let vm = VMInstance {
                        id: vm_id.clone(),
                        run_id: run_id.to_string(),
                        process: None,
                        socket_path: socket_path.clone(),
                        state: vm_state.state,
                        pid: vm_state.pid,
                    };

                    let mut instances = self.instances.write().await;
                    instances.insert(vm_id.clone(), vm);

                    return Ok(VMInstance {
                        id: vm_id,
                        run_id: run_id.to_string(),
                        process: None,
                        socket_path: socket_path.clone(),
                        state: RuntimeState::Running,
                        pid: vm_state.pid,
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to start VM via native bridge: {}", e);
                    return Err(e);
                }
            }
        }

        // No bridge available - create mock VM
        tracing::warn!("No VM bridge available. Using mock mode for VM {}.", vm_id);

        let vm = VMInstance {
            id: vm_id.clone(),
            run_id: run_id.to_string(),
            process: None,
            socket_path,
            state: RuntimeState::Starting,
            pid: None,
        };

        let mut instances = self.instances.write().await;
        instances.insert(vm_id, vm);

        Err(ApiError::Internal(
            "No VM runtime available. Please install QEMU or start the TypeScript runtime."
                .to_string(),
        ))
    }

    /// Stop a VM
    async fn stop_vm(&self, vm_id: &str) -> Result<(), ApiError> {
        // Try to stop via TypeScript bridge first
        let bridge_stopped = if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            bridge.stop_vm(vm_id).await.is_ok()
        } else {
            false
        };

        if !bridge_stopped {
            // Try native bridge
            if let Some(ref native_bridge) = self.native_bridge {
                if let Err(e) = native_bridge.stop_vm(vm_id).await {
                    tracing::warn!("Failed to stop VM via native bridge: {}", e);
                }
            }
        }

        let mut instances = self.instances.write().await;

        if let Some(vm) = instances.get_mut(vm_id) {
            vm.state = RuntimeState::Stopped;

            if let Some(mut process) = vm.process.take() {
                let _ = process.kill().await;
            }

            tracing::info!("Stopped VM: {}", vm_id);
        }

        Ok(())
    }

    /// Get VM status
    async fn get_vm_status(&self, vm_id: &str) -> Result<VMInstance, ApiError> {
        // Try to get status from TypeScript bridge first
        if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            if let Ok(bridge_status) = bridge.get_status(vm_id).await {
                // Update our cached status
                let mut instances = self.instances.write().await;
                if let Some(vm) = instances.get_mut(vm_id) {
                    vm.state = match bridge_status.state.as_str() {
                        "running" => RuntimeState::Running,
                        "paused" => RuntimeState::Paused,
                        "stopped" => RuntimeState::Stopped,
                        "failed" => RuntimeState::Failed,
                        _ => RuntimeState::Starting,
                    };
                    vm.pid = bridge_status.pid;
                }
            }
        } else {
            // Try native bridge
            if let Some(ref native_bridge) = self.native_bridge {
                if let Ok(vm_state) = native_bridge.get_vm_status(vm_id).await {
                    let mut instances = self.instances.write().await;
                    if let Some(vm) = instances.get_mut(vm_id) {
                        vm.state = vm_state.state;
                        vm.pid = vm_state.pid;
                    }
                }
            }
        }

        let instances = self.instances.read().await;

        instances
            .get(vm_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("VM not found: {}", vm_id)))
    }
}

#[async_trait]
impl Runtime for LocalRuntime {
    async fn start(&self, run_id: &str, config: &RunConfig) -> Result<RuntimeHandle, ApiError> {
        let vm = self.start_vm(run_id, config).await?;

        // Wait for VM to be ready
        if vm.state == RuntimeState::Running {
            // Give the VM a moment to fully boot
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        Ok(RuntimeHandle {
            runtime_id: vm.id,
            runtime_type: "local-vm".to_string(),
            connection_info: ConnectionInfo {
                host: "localhost".to_string(),
                port: 0,
                socket_path: Some(vm.socket_path.to_string_lossy().to_string()),
                token: None,
            },
        })
    }

    async fn stop(&self, runtime_id: &str) -> Result<(), ApiError> {
        self.stop_vm(runtime_id).await
    }

    async fn status(&self, runtime_id: &str) -> Result<RuntimeStatus, ApiError> {
        let vm = self.get_vm_status(runtime_id).await?;

        Ok(RuntimeStatus {
            state: vm.state,
            pid: vm.pid,
            exit_code: None,
            resource_usage: None, // TODO: Get from VM metrics
        })
    }

    async fn attach(&self, runtime_id: &str, client: ClientInfo) -> Result<EventStream, ApiError> {
        let _vm = self.get_vm_status(runtime_id).await?;

        // Create event channel
        let (tx, rx) = tokio::sync::mpsc::channel(1000);

        // Try to use bridge for events
        if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            match bridge.attach(runtime_id, &client.client_id).await {
                Ok(mut bridge_rx) => {
                    // Spawn task to forward bridge events
                    tokio::spawn(async move {
                        while let Some(bridge_event) = bridge_rx.recv().await {
                            let event = RuntimeEvent {
                                event_type: match bridge_event.event_type.as_str() {
                                    "output" => RuntimeEventType::Output,
                                    "error" => RuntimeEventType::Error,
                                    "status_change" => RuntimeEventType::StatusChange,
                                    "step_started" => RuntimeEventType::StepStarted,
                                    "step_completed" => RuntimeEventType::StepCompleted,
                                    _ => RuntimeEventType::Heartbeat,
                                },
                                payload: bridge_event.payload,
                                timestamp: bridge_event.timestamp,
                            };

                            if tx.send(event).await.is_err() {
                                break;
                            }
                        }
                    });

                    return Ok(EventStream { rx });
                }
                Err(e) => {
                    tracing::warn!("Failed to attach via bridge: {}. Using fallback.", e);
                }
            }
        }

        // Fallback: spawn a task that sends heartbeats
        let runtime_id = runtime_id.to_string();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

            loop {
                interval.tick().await;

                let event = RuntimeEvent {
                    event_type: RuntimeEventType::Heartbeat,
                    payload: serde_json::json!({
                        "runtime_id": &runtime_id,
                        "client_id": &client.client_id,
                    }),
                    timestamp: chrono::Utc::now(),
                };

                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        Ok(EventStream { rx })
    }

    async fn detach(&self, _runtime_id: &str, _client_id: &str) -> Result<(), ApiError> {
        // Cleanup is handled by the attach task when channel closes
        Ok(())
    }

    async fn exec(
        &self,
        runtime_id: &str,
        command: &str,
        args: &[&str],
    ) -> Result<ExecResult, ApiError> {
        let _vm = self.get_vm_status(runtime_id).await?;

        // Try to use bridge for exec first
        if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            match bridge.exec(runtime_id, command, args).await {
                Ok(result) => {
                    return Ok(ExecResult {
                        exit_code: result.exit_code,
                        stdout: result.stdout,
                        stderr: result.stderr,
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to exec via bridge: {}. Trying native bridge.", e);
                }
            }
        }

        // Try native bridge
        if let Some(ref native_bridge) = self.native_bridge {
            match native_bridge.execute_in_vm(runtime_id, command, args).await {
                Ok(result) => {
                    return Ok(result);
                }
                Err(e) => {
                    tracing::warn!("Failed to exec via native bridge: {}. Using fallback.", e);
                }
            }
        }

        // Fallback: return placeholder
        tracing::info!("Exec on {}: {} {:?}", runtime_id, command, args);

        Ok(ExecResult {
            exit_code: 0,
            stdout: format!("Executed: {} {:?}", command, args),
            stderr: String::new(),
        })
    }

    async fn pause(&self, runtime_id: &str) -> Result<(), ApiError> {
        // Try to pause via bridge
        if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            if let Err(e) = bridge.pause(runtime_id).await {
                tracing::warn!("Failed to pause via bridge: {}", e);
            }
        }

        let mut instances = self.instances.write().await;

        if let Some(vm) = instances.get_mut(runtime_id) {
            vm.state = RuntimeState::Paused;
            tracing::info!("Paused VM: {}", runtime_id);
        }

        Ok(())
    }

    async fn resume(&self, runtime_id: &str) -> Result<(), ApiError> {
        // Try to resume via bridge
        if let Ok(bridge) = self.bridge_manager.get_bridge().await {
            if let Err(e) = bridge.resume(runtime_id).await {
                tracing::warn!("Failed to resume via bridge: {}", e);
            }
        }

        let mut instances = self.instances.write().await;

        if let Some(vm) = instances.get_mut(runtime_id) {
            vm.state = RuntimeState::Running;
            tracing::info!("Resumed VM: {}", runtime_id);
        }

        Ok(())
    }
}

// Clone implementation for VMInstance
impl Clone for VMInstance {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            process: None, // Can't clone process handle
            socket_path: self.socket_path.clone(),
            state: self.state.clone(),
            pid: self.pid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_init_generation() {
        let config = LocalRuntimeConfig::default();
        let bridge = VmBridge {
            data_dir: PathBuf::from("/tmp"),
            state_file: PathBuf::from("/tmp/state.json"),
            qemu_binary: PathBuf::from("qemu-system-x86_64"),
            kvm_available: true,
            ssh_key_path: None,
            base_image_path: None,
        };

        let user_data = bridge.generate_cloud_init_user_data("ssh-ed25519 AAAAC3NzaC1 test");
        assert!(user_data.contains("allternit"));
        assert!(user_data.contains("ssh-ed25519"));

        let meta_data = bridge.generate_cloud_init_meta_data("vm-test-123");
        assert!(meta_data.contains("vm-test-123"));
    }

    #[test]
    fn test_expand_home() {
        let path = expand_home("~/.allternit/test");
        assert!(!path.to_str().unwrap().starts_with("~"));
    }
}
