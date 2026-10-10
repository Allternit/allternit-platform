//! # Rootfs Creation and Management
//!
//! Handles creation of VM root filesystems from OCI images or base filesystems.
//!
//! ## Determinism Features (Phase 2)
//!
//! - **Timestamp Normalization**: All files in the rootfs get the same deterministic timestamp
//!   when an envelope is provided. This ensures reproducible filesystem hashes across runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, error, info, info_span, warn, Instrument};
use walkdir::WalkDir;

use crate::{DriverError, MicroVM};

/// Global counter for unique container names
static CONTAINER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Guard to ensure container cleanup on scope exit
/// Uses synchronous cleanup since Drop cannot be async
struct CleanupGuard {
    cmd: &'static str,
    name: String,
}

impl CleanupGuard {
    fn new(cmd: &'static str, name: impl Into<String>) -> Self {
        Self {
            cmd,
            name: name.into(),
        }
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        // Use synchronous command execution since Drop cannot be async
        // This ensures cleanup happens even on panic
        let _ = std::process::Command::new(self.cmd)
            .args(["rm", "-f", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        debug!(container_name = %self.name, "Cleaned up container");
    }
}

/// Defer cleanup macro - ensures cleanup runs on all exit paths (success, error, panic)
macro_rules! defer_cleanup {
    ($cmd:expr, $name:expr) => {
        let _cleanup = CleanupGuard::new($cmd, $name);
    };
}

/// Rootfs builder for MicroVMs
pub struct RootfsBuilder {
    /// Cache directory for OCI layers
    cache_dir: PathBuf,
}

impl RootfsBuilder {
    /// Create a new rootfs builder
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Create rootfs for a VM from the environment spec
    ///
    /// When `base_time` is provided (deterministic mode), all file timestamps
    /// in the rootfs are normalized to this value for reproducibility.
    #[tracing::instrument(
        skip(self, vm, vm_root_dir, base_time),
        fields(
            vm_id = %vm.id,
            image = %image_ref,
            deterministic = base_time.is_some()
        )
    )]
    pub async fn create_rootfs(
        &self,
        vm: &MicroVM,
        image_ref: &str,
        vm_root_dir: &Path,
        base_time: Option<SystemTime>,
    ) -> Result<PathBuf, DriverError> {
        let rootfs_path = vm_root_dir.join(format!("{}-rootfs.ext4", vm.id));
        let disk_size = vm.vm_config.disk_mib as u64 * 1024 * 1024;

        info!(
            event = "rootfs.create.start",
            vm_id = %vm.id,
            image = %image_ref,
            path = %rootfs_path.display(),
            size_bytes = disk_size,
            disk_mib = vm.vm_config.disk_mib,
            deterministic = base_time.is_some(),
            "Creating rootfs"
        );

        // Create sparse file
        Self::create_sparse_file(&rootfs_path, disk_size).await?;

        // If image is specified, try to extract it; otherwise create minimal rootfs
        if !image_ref.is_empty() && image_ref != "scratch" {
            let vm_id_str = vm.id.to_string();
            match self
                .extract_oci_image(image_ref, &rootfs_path, &vm_id_str, base_time)
                .instrument(info_span!("extract_oci", vm_id = %vm.id, image = %image_ref))
                .await
            {
                Ok(_) => {
                    info!(
                        event = "rootfs.oci.complete",
                        vm_id = %vm.id,
                        image = %image_ref,
                        "OCI image extracted successfully"
                    );
                    return Ok(rootfs_path);
                }
                Err(e) => {
                    warn!(
                        event = "rootfs.oci_failed",
                        vm_id = %vm.id,
                        image = %image_ref,
                        error = %e,
                        "Failed to extract OCI image, falling back to minimal rootfs"
                    );
                }
            }
        }

        // Create minimal rootfs with busybox/alpine
        self.create_minimal_rootfs(&rootfs_path)
            .instrument(info_span!("minimal_rootfs", vm_id = %vm.id))
            .await
            .map_err(|e| {
                error!(
                    event = "rootfs.minimal_failed",
                    vm_id = %vm.id,
                    error = %e,
                    "Failed to create minimal rootfs"
                );
                e
            })?;

        info!(
            event = "rootfs.create.complete",
            vm_id = %vm.id,
            path = %rootfs_path.display(),
            rootfs_type = "minimal",
            "Rootfs created successfully"
        );

        Ok(rootfs_path)
    }

    /// Generate a unique container name
    /// Format: allternit-{vm_id_short}-{timestamp}-{counter}
    fn generate_unique_container_name(vm_id: &str) -> String {
        let vm_id_short = &vm_id[..vm_id.len().min(12)];
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let counter = CONTAINER_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("allternit-{}-{}-{}", vm_id_short, timestamp, counter)
    }

    /// Verify that a container name is available (not in use)
    /// Returns true if the name is free, false if it's already taken
    async fn verify_container_name_available(
        name: &str,
        runtime: &str,
    ) -> Result<bool, DriverError> {
        let output = Command::new(runtime)
            .args(["inspect", name])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run {} inspect: {}", runtime, e),
            })?;

        // If inspect succeeds (exit code 0), container exists (name taken)
        // If inspect fails (non-zero exit), container doesn't exist (name available)
        Ok(!output.status.success())
    }

    /// Create a sparse file for the rootfs
    async fn create_sparse_file(path: &Path, size: u64) -> Result<(), DriverError> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| DriverError::InternalError {
                    message: format!("Failed to create parent directory: {}", e),
                })?;
        }

        // Try fallocate first (fastest), fall back to dd
        let output = Command::new("fallocate")
            .args(["-l", &format!("{}", size), &path.to_string_lossy()])
            .output()
            .await;

        if let Ok(output) = output {
            if output.status.success() {
                return Ok(());
            }
        }

        // Fall back to dd
        let output = Command::new("dd")
            .args([
                "if=/dev/zero",
                &format!("of={}", path.to_string_lossy()),
                "bs=1M",
                &format!("count={}", size / 1024 / 1024),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create rootfs file: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!("dd failed: {}", String::from_utf8_lossy(&output.stderr)),
            });
        }

        Ok(())
    }

    /// Extract OCI image to rootfs
    ///
    /// When `base_time` is provided, normalizes all file timestamps after extraction
    /// to ensure reproducible rootfs hashes across different runs.
    /// 
    /// First checks for packaged VM images (ALLTERNIT_VM_DIR env var) before pulling
    /// from registries.
    #[tracing::instrument(
        skip(self, rootfs_path, base_time),
        fields(image = %image_ref, vm_id)
    )]
    async fn extract_oci_image(
        &self,
        image_ref: &str,
        rootfs_path: &Path,
        vm_id: &str,
        base_time: Option<SystemTime>,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.extract.start",
            image = %image_ref,
            vm_id = %vm_id,
            "Extracting OCI image"
        );

        // Check for packaged VM first (desktop app bundles VMs in resources/vm/)
        if let Ok(packaged_vm_dir) = std::env::var("ALLTERNIT_VM_DIR") {
            let packaged_rootfs = std::path::PathBuf::from(&packaged_vm_dir).join("rootfs.ext4");
            if packaged_rootfs.exists() {
                info!(
                    event = "rootfs.using_packaged",
                    image = %image_ref,
                    path = %packaged_rootfs.display(),
                    "Using packaged VM rootfs"
                );
                
                // Copy packaged rootfs to destination
                let output = Command::new("cp")
                    .args([
                        "--sparse=auto",
                        &packaged_rootfs.to_string_lossy(),
                        &rootfs_path.to_string_lossy(),
                    ])
                    .output()
                    .await
                    .map_err(|e| DriverError::InternalError {
                        message: format!("Failed to copy packaged rootfs: {}", e),
                    })?;
                
                if output.status.success() {
                    info!(
                        event = "rootfs.packaged.complete",
                        image = %image_ref,
                        "Packaged VM rootfs copied successfully"
                    );
                    return Ok(());
                } else {
                    warn!(
                        event = "rootfs.packaged_copy_failed",
                        error = %String::from_utf8_lossy(&output.stderr),
                        "Failed to copy packaged rootfs, falling back to OCI pull"
                    );
                }
            }
        }

        let extract_dir = self
            .cache_dir
            .join("extract")
            .join(blake3::hash(image_ref.as_bytes()).to_hex().to_string());

        // Create extraction directory
        fs::create_dir_all(&extract_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create extract dir: {}", e),
            })?;

        // Try using skopeo + umoci (preferred method)
        if self.command_exists("skopeo").await && self.command_exists("umoci").await {
            info!(
                event = "rootfs.using_skopeo_umoci",
                image = %image_ref,
                "Using skopeo + umoci for OCI extraction"
            );
            self.extract_with_skopeo_umoci(image_ref, &extract_dir)
                .instrument(info_span!("extract_skopeo", image = %image_ref))
                .await
                .map_err(|e| {
                    error!(
                        event = "rootfs.skopeo_failed",
                        image = %image_ref,
                        error = %e,
                        "skopeo/umoci extraction failed"
                    );
                    e
                })?;
        }
        // Try using buildah
        else if self.command_exists("buildah").await {
            info!(
                event = "rootfs.using_buildah",
                image = %image_ref,
                "Using buildah for OCI extraction"
            );
            self.extract_with_buildah(image_ref, &extract_dir, vm_id)
                .instrument(info_span!("extract_buildah", image = %image_ref, vm_id = %vm_id))
                .await
                .map_err(|e| {
                    error!(
                        event = "rootfs.buildah_failed",
                        image = %image_ref,
                        error = %e,
                        "buildah extraction failed"
                    );
                    e
                })?;
        }
        // Try using podman
        else if self.command_exists("podman").await {
            info!(
                event = "rootfs.using_podman",
                image = %image_ref,
                "Using podman for OCI extraction"
            );
            self.extract_with_podman(image_ref, &extract_dir, vm_id)
                .instrument(info_span!("extract_podman", image = %image_ref, vm_id = %vm_id))
                .await
                .map_err(|e| {
                    error!(
                        event = "rootfs.podman_failed",
                        image = %image_ref,
                        error = %e,
                        "podman extraction failed"
                    );
                    e
                })?;
        }
        // Try using docker
        else if self.command_exists("docker").await {
            info!(
                event = "rootfs.using_docker",
                image = %image_ref,
                "Using docker for OCI extraction"
            );
            self.extract_with_docker(image_ref, &extract_dir, vm_id)
                .instrument(info_span!("extract_docker", image = %image_ref, vm_id = %vm_id))
                .await
                .map_err(|e| {
                    error!(
                        event = "rootfs.docker_failed",
                        image = %image_ref,
                        error = %e,
                        "docker extraction failed"
                    );
                    e
                })?;
        } else {
            error!(
                event = "rootfs.no_tool",
                image = %image_ref,
                "No OCI extraction tool found"
            );
            return Err(DriverError::InternalError {
                message: "No OCI extraction tool found (skopeo, buildah, podman, or docker)"
                    .to_string(),
            });
        }

        // Create and mount ext4, copy contents with optional timestamp normalization
        self.copy_to_ext4(&extract_dir, rootfs_path, base_time)
            .instrument(
                info_span!("copy_to_ext4", image = %image_ref, deterministic = base_time.is_some()),
            )
            .await
            .map_err(|e| {
                error!(
                    event = "rootfs.copy_failed",
                    image = %image_ref,
                    error = %e,
                    "Failed to copy to ext4"
                );
                e
            })?;

        info!(
            event = "rootfs.extract.complete",
            image = %image_ref,
            vm_id = %vm_id,
            "OCI image extracted successfully"
        );

        Ok(())
    }

    /// Extract image using skopeo + umoci
    #[tracing::instrument(skip(self, extract_dir), fields(image = %image_ref))]
    async fn extract_with_skopeo_umoci(
        &self,
        image_ref: &str,
        extract_dir: &Path,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.skopeo.start",
            image = %image_ref,
            extract_dir = %extract_dir.display(),
            "Starting skopeo extraction"
        );
        let oci_dir = extract_dir.join("oci");
        let rootfs_dir = extract_dir.join("rootfs");

        fs::create_dir_all(&oci_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create OCI dir: {}", e),
            })?;
        fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create rootfs dir: {}", e),
            })?;

        // Copy image to local OCI layout
        let output = Command::new("skopeo")
            .args([
                "copy",
                &format!("docker://{}", image_ref),
                &format!("oci:{}", oci_dir.display()),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run skopeo: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "skopeo copy failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Unpack OCI layout to rootfs
        let output = Command::new("umoci")
            .args([
                "unpack",
                "--image",
                &format!("{}", oci_dir.display()),
                &format!("{}", rootfs_dir.display()),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run umoci: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "umoci unpack failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        Ok(())
    }

    /// Extract image using buildah
    #[tracing::instrument(skip(self, extract_dir), fields(image = %image_ref, vm_id))]
    async fn extract_with_buildah(
        &self,
        image_ref: &str,
        extract_dir: &Path,
        vm_id: &str,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.buildah.start",
            image = %image_ref,
            vm_id = %vm_id,
            "Starting buildah extraction"
        );
        let rootfs_dir = extract_dir.join("rootfs");
        fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create rootfs dir: {}", e),
            })?;

        // Generate unique container name for this extraction
        let container_name = Self::generate_unique_container_name(vm_id);

        // Pre-flight check: verify container name is available
        if !Self::verify_container_name_available(&container_name, "buildah").await? {
            return Err(DriverError::InternalError {
                message: format!("Container name '{}' is already in use", container_name),
            });
        }

        info!(
            event = "rootfs.buildah.container",
            container_name = %container_name,
            image = %image_ref,
            "Using unique container name for buildah extraction"
        );

        // Create container from image (instead of mounting image directly)
        let output = Command::new("buildah")
            .args(["from", "--name", &container_name, image_ref])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run buildah from: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "buildah from failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Set up cleanup guard to ensure container is removed on all exit paths
        defer_cleanup!("buildah", &container_name);

        // Mount the container
        let output = Command::new("buildah")
            .args(["mount", &container_name])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run buildah mount: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "buildah mount failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let mount_point = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Copy contents
        let output = Command::new("cp")
            .args([
                "-a",
                &format!("{}/.", mount_point),
                &format!("{}/", rootfs_dir.display()),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to copy rootfs: {}", e),
            })?;

        // Unmount (cleanup guard will remove the container)
        let _ = Command::new("buildah")
            .args(["unmount", &container_name])
            .output()
            .await;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: "Failed to copy rootfs contents".to_string(),
            });
        }

        Ok(())
    }

    /// Extract image using podman
    #[tracing::instrument(skip(self, extract_dir), fields(image = %image_ref, vm_id))]
    async fn extract_with_podman(
        &self,
        image_ref: &str,
        extract_dir: &Path,
        vm_id: &str,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.podman.start",
            image = %image_ref,
            vm_id = %vm_id,
            "Starting podman extraction"
        );
        let rootfs_dir = extract_dir.join("rootfs");
        fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create rootfs dir: {}", e),
            })?;

        // Generate unique container name for this extraction
        let container_name = Self::generate_unique_container_name(vm_id);

        // Pre-flight check: verify container name is available
        if !Self::verify_container_name_available(&container_name, "podman").await? {
            return Err(DriverError::InternalError {
                message: format!("Container name '{}' is already in use", container_name),
            });
        }

        info!(
            event = "rootfs.podman.container",
            container_name = %container_name,
            image = %image_ref,
            "Using unique container name for podman extraction"
        );

        // Create container from image
        let output = Command::new("podman")
            .args(["create", "--name", &container_name, image_ref])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run podman create: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "podman create failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Set up cleanup guard to ensure container is removed on all exit paths
        defer_cleanup!("podman", &container_name);

        // Export container filesystem
        let output = Command::new("podman")
            .args([
                "export",
                "-o",
                &format!("{}/rootfs.tar", extract_dir.display()),
                &container_name,
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run podman export: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "podman export failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Extract tar
        let output = Command::new("tar")
            .args([
                "-xf",
                &format!("{}/rootfs.tar", extract_dir.display()),
                "-C",
                &format!("{}", rootfs_dir.display()),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to extract tar: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "tar extraction failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Clean up tar
        let _ = fs::remove_file(format!("{}/rootfs.tar", extract_dir.display())).await;

        // Cleanup guard will remove the container when _cleanup goes out of scope
        Ok(())
    }

    /// Extract image using docker
    #[tracing::instrument(skip(self, extract_dir), fields(image = %image_ref, vm_id))]
    async fn extract_with_docker(
        &self,
        image_ref: &str,
        extract_dir: &Path,
        vm_id: &str,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.docker.start",
            image = %image_ref,
            vm_id = %vm_id,
            "Starting docker extraction"
        );
        let rootfs_dir = extract_dir.join("rootfs");
        fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create rootfs dir: {}", e),
            })?;

        // Generate unique container name for this extraction
        let container_name = Self::generate_unique_container_name(vm_id);

        // Pre-flight check: verify container name is available
        if !Self::verify_container_name_available(&container_name, "docker").await? {
            return Err(DriverError::InternalError {
                message: format!("Container name '{}' is already in use", container_name),
            });
        }

        info!(
            event = "rootfs.docker.container",
            container_name = %container_name,
            image = %image_ref,
            "Using unique container name for docker extraction"
        );

        // Create container from image
        let output = Command::new("docker")
            .args(["create", "--name", &container_name, image_ref])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run docker create: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "docker create failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Set up cleanup guard to ensure container is removed on all exit paths
        defer_cleanup!("docker", &container_name);

        // Export container filesystem
        let output = Command::new("docker")
            .args([
                "export",
                "-o",
                &format!("{}/rootfs.tar", extract_dir.display()),
                &container_name,
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run docker export: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "docker export failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Extract tar
        let output = Command::new("tar")
            .args([
                "-xf",
                &format!("{}/rootfs.tar", extract_dir.display()),
                "-C",
                &format!("{}", rootfs_dir.display()),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to extract tar: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "tar extraction failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Clean up tar
        let _ = fs::remove_file(format!("{}/rootfs.tar", extract_dir.display())).await;

        // Cleanup guard will remove the container when _cleanup goes out of scope
        Ok(())
    }

    /// Copy directory contents to ext4 filesystem
    ///
    /// When `base_time` is provided, normalizes all file timestamps to ensure
    /// reproducible rootfs hashes across different runs.
    #[tracing::instrument(
        skip(self, source_dir, rootfs_path, base_time),
        fields(
            source_dir = %source_dir.display(),
            rootfs_path = %rootfs_path.display(),
            deterministic = base_time.is_some()
        )
    )]
    async fn copy_to_ext4(
        &self,
        source_dir: &Path,
        rootfs_path: &Path,
        base_time: Option<SystemTime>,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.copy_ext4.start",
            source_dir = %source_dir.display(),
            rootfs_path = %rootfs_path.display(),
            deterministic = base_time.is_some(),
            "Copying to ext4 filesystem"
        );
        // Format as ext4
        let output = Command::new("mkfs.ext4")
            .args(["-F", &rootfs_path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to format rootfs: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "mkfs.ext4 failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Mount and copy
        let mount_point = format!("/tmp/allternit-mount-{}", uuid::Uuid::new_v4());
        fs::create_dir_all(&mount_point)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create mount point: {}", e),
            })?;

        // Mount
        let output = Command::new("mount")
            .args([&rootfs_path.to_string_lossy(), mount_point.as_str()])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to mount rootfs: {}", e),
            })?;

        if !output.status.success() {
            // Clean up
            let _ = fs::remove_dir(&mount_point).await;
            return Err(DriverError::InternalError {
                message: format!("mount failed: {}", String::from_utf8_lossy(&output.stderr)),
            });
        }

        // Find actual rootfs (may be in rootfs/ subdirectory from umoci)
        let source_rootfs = if source_dir.join("rootfs").exists() {
            source_dir.join("rootfs")
        } else {
            source_dir.to_path_buf()
        };

        // Copy contents
        let output = Command::new("cp")
            .args([
                "-a",
                &format!("{}/.", source_rootfs.display()),
                &format!("{}/", mount_point),
            ])
            .output()
            .await
            .map_err(|e| {
                let _ = Command::new("umount").arg(&mount_point).output();
                DriverError::InternalError {
                    message: format!("Failed to copy to rootfs: {}", e),
                }
            })?;

        if !output.status.success() {
            let _ = Command::new("umount").arg(&mount_point).output().await;
            let _ = fs::remove_dir(&mount_point).await;
            return Err(DriverError::InternalError {
                message: "Failed to copy rootfs contents".to_string(),
            });
        }

        // Ensure essential directories exist
        for dir in &["dev", "proc", "sys", "tmp", "run", "var", "etc"] {
            let _ = fs::create_dir_all(format!("{}/{}", mount_point, dir)).await;
        }

        // Install the guest agent (+ Xvnc/fluxbox for display sessions) --
        // see docs/research/FIRECRACKER-GUEST-AGENT-VNC-SPEC.md. Best-effort: a
        // rootfs without these still boots and runs (just without a real
        // exec/display channel), matching this function's existing pattern
        // of degrading gracefully rather than failing the whole build.
        if let Err(e) = self.install_guest_agent(&mount_point).await {
            warn!(
                event = "rootfs.guest_agent_install_failed",
                error = %e,
                "Failed to install guest agent into rootfs -- FirecrackerSandbox.run() \
                 will not be able to reach a guest agent in this image"
            );
        }

        // Normalize timestamps if in deterministic mode
        if let Some(time) = base_time {
            if let Err(e) = self.normalize_timestamps(&mount_point, time).await {
                warn!(
                    event = "rootfs.timestamp_warning",
                    error = %e,
                    "Failed to normalize timestamps, continuing without normalization"
                );
                // Don't fail the entire operation for timestamp normalization issues
            } else {
                info!(
                    event = "rootfs.timestamp_normalized",
                    "Timestamps normalized for reproducible rootfs"
                );
            }
        }

        // Unmount
        let _ = Command::new("sync").output().await;
        let _ = Command::new("umount").arg(&mount_point).output().await;
        let _ = fs::remove_dir(&mount_point).await;

        info!(
            event = "rootfs.copy_ext4.complete",
            rootfs_path = %rootfs_path.display(),
            "Ext4 filesystem created successfully"
        );

        Ok(())
    }

    /// Normalize all timestamps in a directory tree to a fixed value
    ///
    /// This ensures reproducible filesystem hashes by setting all file and directory
    /// timestamps to the same deterministic value.
    #[tracing::instrument(skip(self), fields(path, timestamp = ?base_time))]
    async fn normalize_timestamps(
        &self,
        path: &str,
        base_time: SystemTime,
    ) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.normalize.start",
            path = path,
            "Normalizing timestamps for deterministic rootfs"
        );
        let start = std::time::Instant::now();
        let mut files_processed = 0;

        // Convert SystemTime to seconds/nanoseconds for utime
        let duration = base_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| DriverError::InternalError {
                message: "Invalid base time for timestamp normalization".to_string(),
            })?;

        let timestamp_secs = duration.as_secs() as i64;

        // Walk the directory tree
        for entry in WalkDir::new(path) {
            let entry = entry.map_err(|e| DriverError::InternalError {
                message: format!("Failed to read directory entry: {}", e),
            })?;

            let file_path = entry.path();

            // Set both access and modification times
            // Use File::set_times instead of deprecated utime crate
            if let Err(e) = std::fs::File::open(file_path).and_then(|f| {
                let times = std::fs::FileTimes::new()
                    .set_accessed(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(timestamp_secs as u64),
                    )
                    .set_modified(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(timestamp_secs as u64),
                    );
                f.set_times(times)
            }) {
                // Log warning but continue - some files might not be accessible
                debug!("Could not set timestamp for {}: {}", file_path.display(), e);
            }

            files_processed += 1;
        }

        info!(
            event = "rootfs.normalize.complete",
            files_processed = files_processed,
            duration_ms = start.elapsed().as_millis() as u64,
            "Normalized timestamps for reproducible rootfs"
        );

        Ok(())
    }

    /// Create minimal rootfs with busybox
    #[tracing::instrument(skip(self), fields(rootfs_path = %rootfs_path.display()))]
    async fn create_minimal_rootfs(&self, rootfs_path: &Path) -> Result<(), DriverError> {
        debug!(
            event = "rootfs.minimal.start",
            rootfs_path = %rootfs_path.display(),
            "Creating minimal rootfs with busybox"
        );

        // Check for pre-built minimal rootfs
        let minimal_rootfs = self.cache_dir.join("minimal-rootfs.tar.gz");

        if minimal_rootfs.exists() {
            // Extract pre-built rootfs
            self.extract_minimal_rootfs(&minimal_rootfs, rootfs_path)
                .await?;
        } else {
            // Create empty ext4 - guest agent will need to be added separately
            let output = Command::new("mkfs.ext4")
                .args(["-F", &rootfs_path.to_string_lossy()])
                .output()
                .await
                .map_err(|e| DriverError::InternalError {
                    message: format!("Failed to format rootfs: {}", e),
                })?;

            if !output.status.success() {
                return Err(DriverError::InternalError {
                    message: format!(
                        "mkfs.ext4 failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ),
                });
            }
        }

        Ok(())
    }

    /// Extract pre-built minimal rootfs
    async fn extract_minimal_rootfs(
        &self,
        tar_path: &Path,
        rootfs_path: &Path,
    ) -> Result<(), DriverError> {
        // Format as ext4
        let output = Command::new("mkfs.ext4")
            .args(["-F", &rootfs_path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to format rootfs: {}", e),
            })?;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "mkfs.ext4 failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Mount
        let mount_point = format!("/tmp/allternit-mount-{}", uuid::Uuid::new_v4());
        fs::create_dir_all(&mount_point)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create mount point: {}", e),
            })?;

        let output = Command::new("mount")
            .args([&rootfs_path.to_string_lossy(), mount_point.as_str()])
            .output()
            .await
            .map_err(|e| {
                let _ = fs::remove_dir(&mount_point);
                DriverError::InternalError {
                    message: format!("Failed to mount rootfs: {}", e),
                }
            })?;

        if !output.status.success() {
            let _ = fs::remove_dir(&mount_point).await;
            return Err(DriverError::InternalError {
                message: format!("mount failed: {}", String::from_utf8_lossy(&output.stderr)),
            });
        }

        // Extract tar
        let output = Command::new("tar")
            .args(["-xzf", &tar_path.to_string_lossy(), "-C", &mount_point])
            .output()
            .await
            .map_err(|e| {
                let _ = Command::new("umount").arg(&mount_point).output();
                DriverError::InternalError {
                    message: format!("Failed to extract rootfs: {}", e),
                }
            })?;

        if let Err(e) = self.install_guest_agent(&mount_point).await {
            warn!(
                event = "rootfs.guest_agent_install_failed",
                error = %e,
                "Failed to install guest agent into minimal rootfs"
            );
        }

        // Unmount
        let _ = Command::new("sync").output().await;
        let _ = Command::new("umount").arg(&mount_point).output().await;
        let _ = fs::remove_dir(&mount_point).await;

        if !output.status.success() {
            return Err(DriverError::InternalError {
                message: format!(
                    "tar extraction failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        Ok(())
    }

    /// Install the guest agent binary (+ best-effort Xvnc/fluxbox for
    /// display sessions) into an already-mounted rootfs.
    ///
    /// See docs/research/FIRECRACKER-GUEST-AGENT-VNC-SPEC.md. This is what makes
    /// `FirecrackerSandbox.run()` (Python) and `exec_in_vm`/
    /// `start_display_in_vm` (this crate) have a real guest to talk to --
    /// without it, both fail closed (by design) rather than silently running
    /// unsandboxed, but neither actually executes anything in the guest.
    ///
    /// The binary itself is not built here (this crate doesn't cross-compile
    /// `drivers/firecracker-guest-agent` inline) -- it's expected to already
    /// exist at `ALLTERNIT_GUEST_AGENT_BINARY`, following the same
    /// pre-built-artifact convention `extract_oci_image` already uses for
    /// `ALLTERNIT_VM_DIR`. A CI/build step producing that binary (e.g.
    /// `cargo build --release -p allternit-guest-agent --target
    /// x86_64-unknown-linux-musl`) is a prerequisite this method assumes.
    async fn install_guest_agent(&self, mount_point: &str) -> Result<(), DriverError> {
        let agent_binary = std::env::var("ALLTERNIT_GUEST_AGENT_BINARY").map_err(|_| {
            DriverError::InternalError {
                message: "ALLTERNIT_GUEST_AGENT_BINARY not set -- build \
                          drivers/firecracker-guest-agent and point this env var at the \
                          compiled binary for the guest's target architecture"
                    .to_string(),
            }
        })?;

        if !Path::new(&agent_binary).exists() {
            return Err(DriverError::InternalError {
                message: format!("ALLTERNIT_GUEST_AGENT_BINARY does not exist: {agent_binary}"),
            });
        }

        let dest_bin_dir = format!("{mount_point}/usr/local/bin");
        fs::create_dir_all(&dest_bin_dir)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create {dest_bin_dir}: {e}"),
            })?;

        let dest_bin = format!("{dest_bin_dir}/allternit-guest-agent");
        fs::copy(&agent_binary, &dest_bin)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to copy guest agent binary: {e}"),
            })?;

        let chmod_ok = Command::new("chmod")
            .args(["755", &dest_bin])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !chmod_ok {
            warn!(
                event = "rootfs.guest_agent_chmod_failed",
                path = %dest_bin,
                "Failed to chmod guest agent binary -- it may not be executable in the guest"
            );
        }

        // Start-on-boot hook. /etc/rc.local is the most broadly-supported
        // "run this once at the end of boot" mechanism across minimal
        // Debian/Ubuntu/Alpine-derived images without needing to know the
        // base image's specific init system (systemd/OpenRC/sysvinit/none).
        // If the base image doesn't honor rc.local, this line is inert and
        // the agent simply won't start -- start() in firecracker_sandbox.py
        // already fails closed (Ping timeout) rather than assuming success,
        // so that failure mode is caught, not silent.
        let rc_local_path = format!("{mount_point}/etc/rc.local");
        let rc_local_line = "/usr/local/bin/allternit-guest-agent >/var/log/allternit-guest-agent.log 2>&1 &\n";
        let existing = fs::read_to_string(&rc_local_path).await.unwrap_or_default();
        if !existing.contains("allternit-guest-agent") {
            let mut contents = if existing.trim().is_empty() {
                "#!/bin/sh\n".to_string()
            } else {
                existing
            };
            contents.push_str(rc_local_line);
            contents.push_str("exit 0\n");
            fs::write(&rc_local_path, contents)
                .await
                .map_err(|e| DriverError::InternalError {
                    message: format!("Failed to write {rc_local_path}: {e}"),
                })?;
            let _ = Command::new("chmod")
                .args(["755", &rc_local_path])
                .output()
                .await;
        }

        // Best-effort: Xvnc (TigerVNC) + fluxbox for StartDisplay. Not fatal
        // if unavailable -- Execute/Ping/GetMetrics still work without a
        // display; StartDisplay will just fail closed (Xvnc missing) later,
        // consistent with this whole project's fail-closed philosophy.
        let has_apt = Path::new(&format!("{mount_point}/usr/bin/apt-get")).exists();
        if has_apt {
            let install = Command::new("chroot")
                .args([
                    mount_point,
                    "apt-get",
                    "install",
                    "-y",
                    "--no-install-recommends",
                    "tigervnc-standalone-server",
                    "fluxbox",
                ])
                .output()
                .await;
            match install {
                Ok(output) if output.status.success() => {
                    info!(event = "rootfs.display_packages_installed", "Installed Xvnc + fluxbox");
                }
                Ok(output) => {
                    warn!(
                        event = "rootfs.display_packages_failed",
                        stderr = %String::from_utf8_lossy(&output.stderr),
                        "Failed to install Xvnc/fluxbox -- StartDisplay will fail closed in this image"
                    );
                }
                Err(e) => {
                    warn!(
                        event = "rootfs.display_packages_error",
                        error = %e,
                        "chroot apt-get failed to run -- StartDisplay will fail closed in this image"
                    );
                }
            }
        } else {
            debug!(
                event = "rootfs.display_packages_skipped",
                "No apt-get in rootfs -- skipping Xvnc/fluxbox install (non-Debian base image)"
            );
        }

        // Best-effort: bake the Allternit Driver (phase D1b) when the build
        // provides it (ALLTERNIT_DRIVER_DIR, same prebuilt-artifact convention
        // as ALLTERNIT_GUEST_AGENT_BINARY). Structured toolset members
        // (read_ui/act/run_batch/verify) then answer on /run/allternit/driver.sock
        // via rc.local; Execute/Ping still work when this is absent, and the
        // rootfs is published either way (fail-open at the image layer, the
        // same philosophy as the Xvnc block above).
        if let Ok(driver_dir) = std::env::var("ALLTERNIT_DRIVER_DIR") {
            let pkg_src = std::path::Path::new(&driver_dir).join("allternit_driver");
            if pkg_src.is_dir() {
                let dest = format!("{mount_point}/opt/allternit-driver");
                fs::create_dir_all(&dest).await.map_err(|e| DriverError::InternalError {
                    message: format!("Failed to create {dest}: {e}"),
                })?;
                let mut copy = Command::new("cp");
                copy.args(["-R", &format!("{}/", pkg_src.display()), &dest]);
                let copied = copy.output().await.map(|o| o.status.success()).unwrap_or(false);
                if copied {
                    let rpc_src = pkg_src.join("guest_rpc.py");
                    let _ = fs::copy(&rpc_src, format!("{dest}/guest-rpc.py")).await;
                    let rpc_wrapper = format!("{mount_point}/usr/local/bin/allternit-driver-rpc");
                    let _ = fs::create_dir_all(format!("{mount_point}/usr/local/bin")).await;
                    let _ = fs::write(&rpc_wrapper, "#!/bin/sh\nexec /usr/bin/python3 /opt/allternit-driver/guest-rpc.py \"$@\"\n").await;
                    let _ = Command::new("chmod").args(["755", &rpc_wrapper]).output().await;
                    // Start at boot next to the guest agent. The socket dir is
                    // created by the driver itself (mode 0700 under /run).
                    let driver_line = "PYTHONPATH=/opt/allternit-driver /usr/bin/python3 -m allternit_driver --listen unix:/run/allternit/driver.sock --engine auto --state-dir /var/lib/allternit/driver >/var/log/allternit-driver.log 2>&1 &\n";
                    let existing_driver = fs::read_to_string(&rc_local_path).await.unwrap_or_default();
                    if !existing_driver.contains("allternit_driver") {
                        let mut contents = if existing_driver.trim().is_empty() {
                            "#!/bin/sh\n".to_string()
                        } else {
                            existing_driver
                        };
                        contents.push_str(driver_line);
                        let _ = fs::write(&rc_local_path, contents).await;
                        let _ = Command::new("chmod").args(["755", &rc_local_path]).output().await;
                    }
                    info!(event = "rootfs.driver_installed", "Installed the Allternit Driver into the rootfs");
                    if has_apt {
                        let deps = Command::new("chroot")
                            .args([
                                mount_point,
                                "apt-get",
                                "install",
                                "-y",
                                "--no-install-recommends",
                                "python3",
                                "python3-pyatspi",
                                "at-spi2-core",
                                "libatk-adaptor",
                                "libatk-bridge2.0-0",
                                "python3-pil",
                                "xdotool",
                                "scrot",
                            ])
                            .output()
                            .await;
                        match deps {
                            Ok(output) if output.status.success() => {
                                info!(event = "rootfs.driver_deps_installed", "Installed the driver runtime packages");
                            }
                            Ok(output) => {
                                warn!(event = "rootfs.driver_deps_failed", stderr = %String::from_utf8_lossy(&output.stderr), "Driver runtime packages failed -- the driver reports its engines degraded until they exist");
                            }
                            Err(e) => {
                                warn!(event = "rootfs.driver_deps_error", error = %e, "chroot apt-get failed to run for driver deps");
                            }
                        }
                    }
                } else {
                    warn!(event = "rootfs.driver_copy_failed", "Failed to copy the driver package -- structured toolset members stay unavailable in this image");
                }
            } else {
                warn!(event = "rootfs.driver_missing", path = %pkg_src.display(), "ALLTERNIT_DRIVER_DIR has no allternit_driver package");
            }
        }

        Ok(())
    }

    /// Check if a command exists
    async fn command_exists(&self, cmd: &str) -> bool {
        Command::new("which")
            .arg(cmd)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
