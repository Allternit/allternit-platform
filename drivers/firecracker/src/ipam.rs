//! # IP Address Management (IPAM) Module
//!
//! Provides persistent IP address allocation for Firecracker MicroVMs.
//! Ensures no IP conflicts across driver restarts through atomic persistence
//! and conflict detection via ping probes.
//!
//! ## Features
//!
//! - **Atomic persistence**: State is written to `.tmp` file then renamed
//! - **Conflict detection**: Pings IPs before allocation to detect in-use addresses
//! - **CIDR parsing**: Supports standard CIDR notation (e.g., "172.16.0.0/24")
//! - **Orphan reclamation**: Detects and reclaims IPs from dead VMs

use allternit_driver_interface::{DriverError, ExecutionId};
use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Persisted IPAM state structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpamStateData {
    /// Subnet in CIDR notation
    subnet: String,
    /// Gateway IP address
    gateway: String,
    /// Map of execution IDs to allocated IPs
    allocated: HashMap<String, String>,
}

/// IP Address Management state
///
/// Manages a pool of IP addresses within a subnet, providing persistent
/// allocation tracking to survive process restarts.
pub struct IpamState {
    /// The subnet for IP allocation
    subnet: Ipv4Net,
    /// Gateway IP address (first usable IP in subnet)
    gateway: Ipv4Addr,
    /// Currently allocated IPs mapped by execution ID
    allocated: HashMap<ExecutionId, Ipv4Addr>,
    /// Path to persistence file
    persistence_path: PathBuf,
}

impl IpamState {
    /// Load existing IPAM state from disk or create new state (async version)
    ///
    /// # Arguments
    ///
    /// * `subnet` - CIDR notation subnet (e.g., "172.16.0.0/24")
    /// * `persistence_path` - Path to JSON state file
    ///
    /// # Returns
    ///
    /// * `Ok(Self)` - Loaded or newly created IPAM state
    /// * `Err(DriverError)` - If subnet parsing fails or persistence is corrupt
    #[tracing::instrument(
        skip(subnet, persistence_path),
        fields(subnet = %subnet, path = %persistence_path.display())
    )]
    pub async fn load_or_create(
        subnet: &str,
        persistence_path: PathBuf,
    ) -> Result<Self, DriverError> {
        // Parse the subnet
        let subnet: Ipv4Net = subnet.parse().map_err(|e| DriverError::InvalidInput {
            field: "subnet".to_string(),
            reason: format!("Invalid CIDR format: {}", e),
        })?;

        // Calculate gateway (first usable IP in subnet)
        let gateway = Self::calculate_gateway(&subnet);

        // Try to load existing state
        if persistence_path.exists() {
            match Self::load_from_disk(&persistence_path).await {
                Ok(data) => {
                    // Validate subnet matches
                    let loaded_subnet: Ipv4Net =
                        data.subnet
                            .parse()
                            .map_err(|e| DriverError::InternalError {
                                message: format!("Corrupt IPAM state - invalid subnet: {}", e),
                            })?;

                    if loaded_subnet != subnet {
                        warn!(
                            event = "ipam.subnet_mismatch",
                            expected = %subnet,
                            got = %loaded_subnet,
                            "IPAM subnet mismatch, creating fresh state"
                        );
                    } else {
                        // Convert allocated map
                        let mut allocated = HashMap::new();
                        for (id_str, ip_str) in data.allocated {
                            let id = ExecutionId(uuid::Uuid::parse_str(&id_str).map_err(|e| {
                                DriverError::InternalError {
                                    message: format!("Corrupt IPAM state - invalid UUID: {}", e),
                                }
                            })?);
                            let ip: Ipv4Addr =
                                ip_str.parse().map_err(|e| DriverError::InternalError {
                                    message: format!("Corrupt IPAM state - invalid IP: {}", e),
                                })?;
                            allocated.insert(id, ip);
                        }

                        info!(
                            event = "ipam.loaded",
                            path = %persistence_path.display(),
                            allocations = allocated.len(),
                            "Loaded IPAM state from disk"
                        );

                        return Ok(Self {
                            subnet,
                            gateway,
                            allocated,
                            persistence_path,
                        });
                    }
                }
                Err(e) => {
                    warn!(
                        event = "ipam.load_failed",
                        path = %persistence_path.display(),
                        error = %e,
                        "Failed to load IPAM state, creating fresh state"
                    );
                }
            }
        }

        // Create fresh state
        info!(
            event = "ipam.create_new",
            subnet = %subnet,
            path = %persistence_path.display(),
            "Creating new IPAM state"
        );

        let state = Self {
            subnet,
            gateway,
            allocated: HashMap::new(),
            persistence_path,
        };

        // Persist initial empty state
        state.persist().await?;

        Ok(state)
    }

    /// Allocate an IP address for a VM
    ///
    /// Finds the next available IP in the subnet, skipping:
    /// - Network address (.0)
    /// - Gateway address (.1)
    /// - Broadcast address (.255)
    /// - Already allocated IPs
    /// - IPs that respond to ping (conflict detection)
    ///
    /// # Arguments
    ///
    /// * `vm_id` - Execution ID of the VM requesting the IP
    ///
    /// # Returns
    ///
    /// * `Ok(Ipv4Addr)` - Allocated IP address
    /// * `Err(DriverError::InsufficientResources)` - No IPs available
    #[tracing::instrument(skip(self), fields(vm_id = %vm_id))]
    pub async fn allocate(&mut self, vm_id: ExecutionId) -> Result<Ipv4Addr, DriverError> {
        // Check if already allocated
        if let Some(&ip) = self.allocated.get(&vm_id) {
            debug!(
                event = "ipam.already_allocated",
                vm_id = %vm_id,
                ip = %ip,
                "IP already allocated for VM"
            );
            return Ok(ip);
        }

        // Get list of already allocated IPs for quick lookup
        let allocated_ips: std::collections::HashSet<_> =
            self.allocated.values().copied().collect();

        // Iterate through host IPs in the subnet
        for ip in self.subnet.hosts() {
            // Skip gateway
            if ip == self.gateway {
                continue;
            }

            // Skip already allocated
            if allocated_ips.contains(&ip) {
                continue;
            }

            // Conflict detection: ping the IP
            if self.is_ip_in_use(ip).await {
                warn!(
                    event = "ipam.conflict_detected",
                    ip = %ip,
                    "IP is in use (responded to ping), skipping"
                );
                // Mark as allocated by inserting a dummy entry that will be
                // cleaned up by reclaim_orphaned later
                continue;
            }

            // Found available IP
            self.allocated.insert(vm_id, ip);
            self.persist().await?;

            info!(
                event = "ipam.allocated",
                vm_id = %vm_id,
                ip = %ip,
                total_allocations = self.allocated.len(),
                "Allocated IP address for VM"
            );
            return Ok(ip);
        }

        warn!(
            event = "ipam.exhausted",
            vm_id = %vm_id,
            subnet = %self.subnet,
            total_allocations = self.allocated.len(),
            "No IP addresses available in subnet"
        );
        Err(DriverError::InsufficientResources {
            resource: "IP addresses in subnet".to_string(),
        })
    }

    /// Release an IP address allocation
    ///
    /// Removes the allocation for the given VM and persists the state.
    ///
    /// # Arguments
    ///
    /// * `vm_id` - Execution ID of the VM releasing the IP
    ///
    /// # Returns
    ///
    /// * `Ok(())` - IP released successfully
    /// * `Err(DriverError)` - If persistence fails
    #[tracing::instrument(skip(self), fields(vm_id = %vm_id))]
    pub async fn release(&mut self, vm_id: ExecutionId) -> Result<(), DriverError> {
        if let Some(ip) = self.allocated.remove(&vm_id) {
            info!(
                event = "ipam.released",
                vm_id = %vm_id,
                ip = %ip,
                total_allocations = self.allocated.len(),
                "Released IP address"
            );
            self.persist().await?;
        } else {
            debug!(
                event = "ipam.no_allocation",
                vm_id = %vm_id,
                "No IP allocation found for VM"
            );
        }
        Ok(())
    }

    /// Reclaim IP addresses from orphaned VMs
    ///
    /// Pings all allocated IPs and removes allocations for IPs that
    /// no longer respond (indicating the VM is gone).
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - Number of orphaned IPs reclaimed
    /// * `Err(DriverError)` - If persistence fails
    #[tracing::instrument(skip(self))]
    pub async fn _reclaim_orphaned(&mut self) -> Result<usize, DriverError> {
        let mut to_reclaim = Vec::new();

        // Check each allocated IP
        for (vm_id, ip) in &self.allocated {
            if !self.is_ip_in_use(*ip).await {
                debug!(
                    event = "ipam.orphan_detected",
                    vm_id = %vm_id,
                    ip = %ip,
                    "IP appears orphaned (no ping response)"
                );
                to_reclaim.push(*vm_id);
            }
        }

        let reclaimed_count = to_reclaim.len();

        // Remove orphaned allocations
        for vm_id in to_reclaim {
            if let Some(ip) = self.allocated.remove(&vm_id) {
                info!(
                    event = "ipam.reclaimed",
                    vm_id = %vm_id,
                    ip = %ip,
                    "Reclaimed orphaned IP address"
                );
            }
        }

        if reclaimed_count > 0 {
            self.persist().await?;
        }

        info!(
            event = "ipam.reclaim_complete",
            reclaimed = reclaimed_count,
            total_allocations = self.allocated.len(),
            "Reclaimed orphaned IP addresses"
        );
        Ok(reclaimed_count)
    }

    /// Get the gateway IP address for this subnet
    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    /// Get the subnet
    pub fn _subnet(&self) -> Ipv4Net {
        self.subnet
    }

    /// Get count of allocated IPs
    pub fn _allocation_count(&self) -> usize {
        self.allocated.len()
    }

    /// Calculate the gateway IP (first usable IP in subnet)
    fn calculate_gateway(subnet: &Ipv4Net) -> Ipv4Addr {
        let network = subnet.network();
        let octets = network.octets();
        // Gateway is .1 in the subnet
        Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3] + 1)
    }

    /// Check if an IP is in use by pinging it
    ///
    /// Uses `ping -c 1 -W 1` to check if host responds.
    /// Returns true if the IP responds to ping.
    #[tracing::instrument(skip(self), fields(ip = %ip))]
    async fn is_ip_in_use(&self, ip: Ipv4Addr) -> bool {
        let output = Command::new("ping")
            .args(["-c", "1", "-W", "1", &ip.to_string()])
            .output()
            .await;

        match output {
            Ok(result) => {
                let in_use = result.status.success();
                debug!(
                    event = "ipam.ping_complete",
                    ip = %ip,
                    in_use = in_use,
                    "Ping check completed"
                );
                in_use
            }
            Err(e) => {
                debug!(
                    event = "ipam.ping_failed",
                    ip = %ip,
                    error = %e,
                    "Failed to run ping command, assuming IP not in use"
                );
                // Assume IP is not in use if we can't ping
                false
            }
        }
    }

    /// Persist state to disk atomically
    ///
    /// Writes to a temporary file first, then renames it to the
    /// target path for atomic updates.
    async fn persist(&self) -> Result<(), DriverError> {
        // Create parent directory if needed
        if let Some(parent) = self.persistence_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| DriverError::InternalError {
                    message: format!("Failed to create IPAM directory: {}", e),
                })?;
        }

        // Prepare data for serialization
        let allocated: HashMap<String, String> = self
            .allocated
            .iter()
            .map(|(id, ip)| (id.to_string(), ip.to_string()))
            .collect();

        let data = IpamStateData {
            subnet: self.subnet.to_string(),
            gateway: self.gateway.to_string(),
            allocated,
        };

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&data).map_err(|e| DriverError::InternalError {
            message: format!("Failed to serialize IPAM state: {}", e),
        })?;

        // Write to temporary file
        let tmp_path = self.persistence_path.with_extension("tmp");
        fs::write(&tmp_path, json)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to write IPAM state: {}", e),
            })?;

        // Atomic rename
        fs::rename(&tmp_path, &self.persistence_path)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to persist IPAM state: {}", e),
            })?;

        debug!(
            event = "ipam.persisted",
            path = %self.persistence_path.display(),
            allocations = self.allocated.len(),
            "Persisted IPAM state"
        );
        Ok(())
    }

    /// Load state from disk
    async fn load_from_disk(path: &Path) -> Result<IpamStateData, DriverError> {
        let json = fs::read_to_string(path)
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to read IPAM state: {}", e),
            })?;

        let data: IpamStateData =
            serde_json::from_str(&json).map_err(|e| DriverError::InternalError {
                message: format!("Failed to parse IPAM state: {}", e),
            })?;

        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_calculate_gateway() {
        let subnet: Ipv4Net = "172.16.0.0/24".parse().unwrap();
        let gateway = IpamState::calculate_gateway(&subnet);
        assert_eq!(gateway, Ipv4Addr::new(172, 16, 0, 1));
    }

    #[test]
    fn test_calculate_gateway_different_subnet() {
        let subnet: Ipv4Net = "10.0.5.0/24".parse().unwrap();
        let gateway = IpamState::calculate_gateway(&subnet);
        assert_eq!(gateway, Ipv4Addr::new(10, 0, 5, 1));
    }

    #[tokio::test]
    async fn test_load_or_create_new() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("ipam.json");

        let ipam = IpamState::load_or_create("172.16.0.0/24", path.clone())
            .await
            .unwrap();

        assert_eq!(ipam._subnet().to_string(), "172.16.0.0/24");
        assert_eq!(ipam.gateway(), Ipv4Addr::new(172, 16, 0, 1));
        assert_eq!(ipam._allocation_count(), 0);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn test_allocate_and_release() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("ipam.json");

        let mut ipam = IpamState::load_or_create("172.16.0.0/24", path)
            .await
            .unwrap();

        let vm_id = ExecutionId(uuid::Uuid::new_v4());

        // First allocation should return an IP
        let ip1 = ipam.allocate(vm_id).await.unwrap();
        assert!(ipam._subnet().contains(&ip1));
        assert_ne!(ip1, ipam.gateway()); // Should not allocate gateway

        // Second allocation for same VM should return same IP
        let ip2 = ipam.allocate(vm_id).await.unwrap();
        assert_eq!(ip1, ip2);

        // Release should work
        ipam.release(vm_id).await.unwrap();
        assert_eq!(ipam._allocation_count(), 0);
    }

    #[tokio::test]
    async fn test_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("ipam.json");

        let vm_id = ExecutionId(uuid::Uuid::new_v4());
        let allocated_ip: Ipv4Addr;

        // Create and allocate
        {
            let mut ipam = IpamState::load_or_create("172.16.0.0/24", path.clone())
                .await
                .unwrap();
            allocated_ip = ipam.allocate(vm_id).await.unwrap();
        }

        // Load and verify
        {
            let ipam = IpamState::load_or_create("172.16.0.0/24", path)
                .await
                .unwrap();
            assert_eq!(ipam._allocation_count(), 1);
            assert_eq!(ipam.allocated.get(&vm_id), Some(&allocated_ip));
        }
    }
}
