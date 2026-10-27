//! Network Policy Enforcement for Firecracker MicroVMs
//!
//! This module provides network isolation and traffic control for MicroVM TAP devices
//! using Linux Traffic Control (tc) for rate limiting and iptables for firewall rules.

use allternit_driver_interface::{DriverError, NetworkPolicy};
use tokio::process::Command;
use tracing::Instrument;
use tracing::{debug, error, info, info_span, warn};

/// Network policy enforcer for a TAP device
///
/// Manages tc (traffic control) qdiscs and iptables rules to enforce:
/// - Rate limiting (egress bandwidth)
/// - Egress allow/deny
/// - Allowed hosts whitelist
/// - DNS blocking
#[derive(Debug, Clone)]
pub struct NetworkPolicyEnforcer {
    tap_name: String,
    policy: NetworkPolicy,
    rate_limit_kbps: Option<u64>,
}

impl NetworkPolicyEnforcer {
    /// Create a new network policy enforcer for the given TAP device
    pub fn new(tap_name: String, policy: NetworkPolicy) -> Self {
        Self {
            tap_name,
            policy,
            rate_limit_kbps: None,
        }
    }

    /// Create a new enforcer with rate limiting
    pub fn with_rate_limit(tap_name: String, policy: NetworkPolicy, rate_limit_kbps: u64) -> Self {
        Self {
            tap_name,
            policy,
            rate_limit_kbps: Some(rate_limit_kbps),
        }
    }

    /// Get the TAP device name
    pub fn _tap_name(&self) -> &str {
        &self.tap_name
    }

    /// Apply all network policies to the TAP device
    ///
    /// This method:
    /// 1. Sets up HTB qdisc for rate limiting (if configured)
    /// 2. Applies iptables FORWARD rules for egress control
    /// 3. Configures DNS filtering
    #[tracing::instrument(
        skip(self),
        fields(
            tap = %self.tap_name,
            egress_allowed = self.policy.egress_allowed,
            dns_allowed = self.policy.dns_allowed,
            rate_limit_kbps = ?self.rate_limit_kbps
        )
    )]
    pub async fn apply(&self) -> Result<(), DriverError> {
        info!(
            event = "netpolicy.apply.start",
            tap = %self.tap_name,
            allowed_hosts = ?self.policy.allowed_hosts,
            allowed_ports = ?self.policy.allowed_ports,
            "Applying network policy"
        );

        // Apply rate limiting first
        if let Some(rate) = self.rate_limit_kbps {
            self.apply_rate_limit(rate)
                .instrument(info_span!("apply_rate_limit", tap = %self.tap_name, rate_kbps = rate))
                .await
                .map_err(|e| {
                    error!(
                        event = "netpolicy.rate_limit_failed",
                        error = %e,
                        tap = %self.tap_name,
                        rate_kbps = rate,
                        "Failed to apply rate limiting"
                    );
                    e
                })?;
        }

        // Apply iptables rules for egress control
        self.apply_iptables_rules()
            .instrument(info_span!("apply_iptables", tap = %self.tap_name))
            .await
            .map_err(|e| {
                error!(
                    event = "netpolicy.iptables_failed",
                    error = %e,
                    tap = %self.tap_name,
                    "Failed to apply iptables rules"
                );
                e
            })?;

        info!(
            event = "netpolicy.apply.complete",
            tap = %self.tap_name,
            "Network policy applied successfully"
        );
        Ok(())
    }

    /// Remove all network policies from the TAP device
    ///
    /// This method attempts to clean up all rules even if some fail.
    /// Errors are logged but only returned if all cleanup attempts fail.
    #[tracing::instrument(skip(self), fields(tap = %self.tap_name))]
    pub async fn remove(&self) -> Result<(), DriverError> {
        info!(
            event = "netpolicy.remove.start",
            tap = %self.tap_name,
            "Removing network policy"
        );

        let mut last_error = None;

        // Remove tc rules
        if let Err(e) = self.remove_rate_limit().await {
            warn!(
                event = "netpolicy.rate_limit_remove_warning",
                tap = %self.tap_name,
                error = %e,
                "Failed to remove tc rate limit"
            );
            last_error = Some(e);
        }

        // Remove iptables rules
        if let Err(e) = self.remove_iptables_rules().await {
            warn!(
                event = "netpolicy.iptables_remove_warning",
                tap = %self.tap_name,
                error = %e,
                "Failed to remove iptables rules"
            );
            last_error = Some(e);
        }

        if let Some(e) = last_error {
            return Err(e);
        }

        info!(
            event = "netpolicy.remove.complete",
            tap = %self.tap_name,
            "Network policy removed successfully"
        );
        Ok(())
    }

    //==========================================================================
    // Traffic Control (TC) Methods
    //==========================================================================

    /// Apply HTB rate limiting to the TAP device
    ///
    /// Creates an HTB qdisc with a single class that limits egress bandwidth.
    #[tracing::instrument(skip(self), fields(tap = %self.tap_name, rate_kbps))]
    async fn apply_rate_limit(&self, rate_kbps: u64) -> Result<(), DriverError> {
        debug!(
            event = "netpolicy.rate_limit.start",
            tap = %self.tap_name,
            rate_kbps = rate_kbps,
            "Applying rate limit"
        );

        // Delete any existing qdisc first (ignore errors)
        let _ = self.remove_rate_limit().await;

        // Add HTB qdisc at root
        let output = Command::new("tc")
            .args([
                "qdisc",
                "add",
                "dev",
                &self.tap_name,
                "root",
                "handle",
                "1:",
                "htb",
                "default",
                "1",
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to add HTB qdisc: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check if it's just a "file exists" error from previous partial setup
            if stderr.contains("File exists") {
                warn!(
                    event = "netpolicy.qdisc_exists",
                    tap = %self.tap_name,
                    "HTB qdisc already exists, attempting to replace"
                );
                // Try to replace instead
                let _ = self.remove_rate_limit().await;
                return Box::pin(self.apply_rate_limit(rate_kbps)).await;
            }
            error!(
                event = "netpolicy.qdisc_failed",
                tap = %self.tap_name,
                error = %stderr,
                "Failed to add HTB qdisc"
            );
            return Err(DriverError::InternalError {
                message: format!("tc qdisc add failed: {}", stderr),
            });
        }

        // Calculate burst (min 32k, max 1% of rate)
        let burst = (rate_kbps / 32).max(32).min(rate_kbps / 100);

        // Add HTB class with rate limit
        let output = Command::new("tc")
            .args([
                "class",
                "add",
                "dev",
                &self.tap_name,
                "parent",
                "1:",
                "classid",
                "1:1",
                "htb",
                "rate",
                &format!("{}kbit", rate_kbps),
                "burst",
                &format!("{}k", burst),
            ])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to add HTB class: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DriverError::InternalError {
                message: format!("tc class add failed: {}", stderr),
            });
        }

        debug!(
            event = "netpolicy.rate_limit.complete",
            tap = %self.tap_name,
            rate_kbps = rate_kbps,
            burst = burst,
            "Rate limit applied"
        );
        Ok(())
    }

    /// Remove rate limiting qdisc from the TAP device
    #[tracing::instrument(skip(self), fields(tap = %self.tap_name))]
    async fn remove_rate_limit(&self) -> Result<(), DriverError> {
        debug!(
            event = "netpolicy.rate_limit_remove.start",
            tap = %self.tap_name,
            "Removing rate limit"
        );

        let output = Command::new("tc")
            .args(["qdisc", "del", "dev", &self.tap_name, "root"])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to remove tc qdisc: {}", e),
            })?;

        // Ignore "No such file or directory" errors (qdisc doesn't exist)
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("No such file or directory") {
                return Err(DriverError::InternalError {
                    message: format!("tc qdisc del failed: {}", stderr),
                });
            }
        }

        Ok(())
    }

    //==========================================================================
    // iptables Methods
    //==========================================================================

    /// Apply iptables FORWARD rules for egress control
    ///
    /// Rules are applied in order of specificity:
    /// 1. Allow established connections
    /// 2. Allow specific hosts (if whitelist configured)
    /// 3. Allow DNS (if enabled)
    /// 4. Drop all other egress (if egress not allowed)
    #[tracing::instrument(skip(self), fields(tap = %self.tap_name))]
    async fn apply_iptables_rules(&self) -> Result<(), DriverError> {
        debug!(
            event = "netpolicy.iptables.start",
            tap = %self.tap_name,
            "Applying iptables rules"
        );

        // Create a custom chain for this TAP device (for easier cleanup)
        let chain_name = self.iptables_chain_name();

        // Create chain if it doesn't exist
        let _ = self.create_iptables_chain(&chain_name).await;

        // Flush chain to remove any existing rules
        self.flush_iptables_chain(&chain_name).await?;

        // Add rules to the custom chain
        self.populate_iptables_chain(&chain_name).await?;

        // Jump from FORWARD chain to our custom chain
        // First check if rule already exists
        if !self
            .iptables_rule_exists("FORWARD", &["-i", &self.tap_name, "-j", &chain_name])
            .await?
        {
            self.run_iptables(&[
                "-I",
                "FORWARD",
                "1",
                "-i",
                &self.tap_name,
                "-j",
                &chain_name,
            ])
            .await?;
        }

        debug!(
            event = "netpolicy.iptables.complete",
            tap = %self.tap_name,
            chain = %chain_name,
            "iptables rules applied"
        );
        Ok(())
    }

    /// Populate the custom iptables chain with policy rules
    async fn populate_iptables_chain(&self, chain_name: &str) -> Result<(), DriverError> {
        // Allow established and related connections (for return traffic)
        self.run_iptables(&[
            "-A",
            chain_name,
            "-m",
            "state",
            "--state",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ])
        .await?;

        // Allow loopback (just in case)
        self.run_iptables(&["-A", chain_name, "-o", "lo", "-j", "ACCEPT"])
            .await?;

        if self.policy.egress_allowed {
            // Egress is allowed, but we may have restrictions

            // If allowed_hosts is specified, restrict to those hosts
            if !self.policy.allowed_hosts.is_empty()
                && !self.policy.allowed_hosts.contains(&"*".to_string())
            {
                // First allow specified hosts
                for host in &self.policy.allowed_hosts {
                    if let Err(e) = self
                        .run_iptables(&["-A", chain_name, "-d", host, "-j", "ACCEPT"])
                        .await
                    {
                        warn!(tap = %self.tap_name, host = %host, error = %e, "Failed to add host allow rule");
                    }
                }

                // Block DNS if not allowed
                if !self.policy.dns_allowed {
                    self.run_iptables(&[
                        "-A", chain_name, "-p", "udp", "--dport", "53", "-j", "DROP",
                    ])
                    .await?;
                    self.run_iptables(&[
                        "-A", chain_name, "-p", "tcp", "--dport", "53", "-j", "DROP",
                    ])
                    .await?;
                }

                // Drop everything else
                self.run_iptables(&["-A", chain_name, "-j", "DROP"]).await?;
            } else {
                // No host restrictions, but check DNS
                if !self.policy.dns_allowed {
                    // Block DNS before accepting
                    self.run_iptables(&[
                        "-A", chain_name, "-p", "udp", "--dport", "53", "-j", "DROP",
                    ])
                    .await?;
                    self.run_iptables(&[
                        "-A", chain_name, "-p", "tcp", "--dport", "53", "-j", "DROP",
                    ])
                    .await?;
                }

                // Allow all other egress
                self.run_iptables(&["-A", chain_name, "-j", "ACCEPT"])
                    .await?;
            }
        } else {
            // Egress is completely denied

            // Block DNS if not allowed (redundant but explicit)
            if !self.policy.dns_allowed {
                self.run_iptables(&["-A", chain_name, "-p", "udp", "--dport", "53", "-j", "DROP"])
                    .await?;
                self.run_iptables(&["-A", chain_name, "-p", "tcp", "--dport", "53", "-j", "DROP"])
                    .await?;
            }

            // Drop all forwarding from this interface
            self.run_iptables(&["-A", chain_name, "-j", "DROP"]).await?;
        }

        // Handle port restrictions if specified
        if !self.policy.allowed_ports.is_empty() {
            // If we have port restrictions, we need to insert them before the final ACCEPT/DROP
            // For simplicity, we handle this by re-populating the chain
            // In production, you might want more sophisticated rule management
            for port in &self.policy.allowed_ports {
                // Allow specific destination ports
                self.run_iptables(&[
                    "-I",
                    chain_name,
                    "2", // Insert after ESTABLISHED rule
                    "-p",
                    "tcp",
                    "--dport",
                    &port.to_string(),
                    "-j",
                    "ACCEPT",
                ])
                .await?;
            }
        }

        Ok(())
    }

    /// Remove all iptables rules for this TAP device
    #[tracing::instrument(skip(self), fields(tap = %self.tap_name))]
    async fn remove_iptables_rules(&self) -> Result<(), DriverError> {
        debug!(
            event = "netpolicy.iptables_remove.start",
            tap = %self.tap_name,
            "Removing iptables rules"
        );

        let chain_name = self.iptables_chain_name();

        // Remove jump rule from FORWARD chain
        let _ = self
            .run_iptables(&["-D", "FORWARD", "-i", &self.tap_name, "-j", &chain_name])
            .await;

        // Flush the custom chain
        let _ = self.flush_iptables_chain(&chain_name).await;

        // Delete the custom chain
        let _ = self.delete_iptables_chain(&chain_name).await;

        Ok(())
    }

    //==========================================================================
    // iptables Helper Methods
    //==========================================================================

    /// Generate a unique chain name for this TAP device
    fn iptables_chain_name(&self) -> String {
        format!("Allternit-{}", self.tap_name.to_uppercase().replace("-", "_"))
    }

    /// Create a custom iptables chain
    async fn create_iptables_chain(&self, chain_name: &str) -> Result<(), DriverError> {
        let output = Command::new("iptables")
            .args(["-N", chain_name, "-w"])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to create iptables chain: {}", e),
            })?;

        // Ignore "Chain already exists" error
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("Chain already exists") {
                return Err(DriverError::InternalError {
                    message: format!("iptables -N failed: {}", stderr),
                });
            }
        }

        Ok(())
    }

    /// Flush all rules from a custom chain
    async fn flush_iptables_chain(&self, chain_name: &str) -> Result<(), DriverError> {
        let output = Command::new("iptables")
            .args(["-F", chain_name, "-w"])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to flush iptables chain: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DriverError::InternalError {
                message: format!("iptables -F failed: {}", stderr),
            });
        }

        Ok(())
    }

    /// Delete a custom iptables chain
    async fn delete_iptables_chain(&self, chain_name: &str) -> Result<(), DriverError> {
        let output = Command::new("iptables")
            .args(["-X", chain_name, "-w"])
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to delete iptables chain: {}", e),
            })?;

        // Ignore "No chain/target/match by that name" error
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("No chain/target/match by that name") {
                return Err(DriverError::InternalError {
                    message: format!("iptables -X failed: {}", stderr),
                });
            }
        }

        Ok(())
    }

    /// Run an iptables command with the given arguments
    async fn run_iptables(&self, args: &[&str]) -> Result<(), DriverError> {
        debug!(command = %format!("iptables {}", args.join(" ")), "Running iptables");

        let output = Command::new("iptables")
            .args(args)
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to run iptables: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "Rule does not exist" errors during cleanup
            if args.contains(&"-D") && stderr.contains("No chain/target/match by that name") {
                return Ok(());
            }
            return Err(DriverError::InternalError {
                message: format!("iptables {} failed: {}", args.join(" "), stderr),
            });
        }

        Ok(())
    }

    /// Check if an iptables rule exists
    async fn iptables_rule_exists(&self, chain: &str, rule: &[&str]) -> Result<bool, DriverError> {
        let mut args = vec!["-C", chain];
        args.extend_from_slice(rule);
        args.push("-w");

        let output = Command::new("iptables")
            .args(&args)
            .output()
            .await
            .map_err(|e| DriverError::InternalError {
                message: format!("Failed to check iptables rule: {}", e),
            })?;

        Ok(output.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_name_generation() {
        let policy = NetworkPolicy::default();
        let enforcer = NetworkPolicyEnforcer::new("tap-1234".to_string(), policy);
        assert_eq!(enforcer.iptables_chain_name(), "Allternit-TAP_1234");
    }

    #[test]
    fn test_enforcer_creation() {
        let policy = NetworkPolicy {
            egress_allowed: false,
            allowed_hosts: vec!["10.0.0.0/8".to_string()],
            allowed_ports: vec![80, 443],
            dns_allowed: true,
        };

        let enforcer = NetworkPolicyEnforcer::with_rate_limit("tap-test".to_string(), policy, 1000);

        assert_eq!(enforcer._tap_name(), "tap-test");
        assert_eq!(enforcer.rate_limit_kbps, Some(1000));
        assert!(!enforcer.policy.egress_allowed);
    }
}
