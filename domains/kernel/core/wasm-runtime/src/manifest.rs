//! Capsule Manifest schema inspired by Extism.
//!
//! Defines the security and configuration manifest for WASM capsules.

use crate::capabilities::{Capability, CapabilityGrant};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

/// The main Capsule Manifest schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsuleManifest {
    /// Name of the capsule
    pub name: String,

    /// Version of the capsule
    pub version: String,

    /// Optional description
    pub description: Option<String>,

    /// The WASM module or component
    pub module: ModuleSource,

    /// Configuration for the capsule
    pub config: Option<HashMap<String, String>>,

    /// Allowed hosts for HTTP requests
    #[serde(default)]
    pub allowed_hosts: Vec<String>,

    /// Allowed paths for file system access
    #[serde(default)]
    pub allowed_paths: HashMap<String, String>,

    /// Environment variables to expose
    #[serde(default)]
    pub allowed_env: Vec<String>,

    /// Memory limits in bytes
    pub memory: Option<MemoryConfig>,

    /// Timeout configuration in milliseconds
    pub timeout: Option<u64>,

    /// Function definitions for host functions
    #[serde(default)]
    pub functions: Vec<FunctionDefinition>,

    /// Metadata for the capsule
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Source of the WASM module
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum ModuleSource {
    /// WASM file at a local path
    File { path: String },

    /// WASM module from a URL
    Url { url: String },

    /// WASM module from a registry
    Registry { name: String, version: String },

    /// Inline WASM bytes (base64 encoded)
    Inline { data: String },
}

/// Memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Initial memory pages (64KB each)
    pub initial_pages: Option<u32>,

    /// Maximum memory pages (64KB each)
    pub max_pages: Option<u32>,

    /// Maximum memory in bytes
    pub max_bytes: Option<u64>,
}

/// Function definition for host functions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Name of the function
    pub name: String,

    /// Parameters of the function
    pub params: Vec<String>,

    /// Return type of the function
    pub result: Option<String>,

    /// Optional description
    pub description: Option<String>,
}

impl CapsuleManifest {
    /// Create a new minimal manifest
    pub fn new(name: String, version: String, module: ModuleSource) -> Self {
        Self {
            name,
            version,
            description: None,
            module,
            config: None,
            allowed_hosts: vec![],
            allowed_paths: HashMap::new(),
            allowed_env: vec![],
            memory: None,
            timeout: None,
            functions: vec![],
            metadata: HashMap::new(),
        }
    }

    /// Convert the manifest to a capability grant
    pub fn to_capability_grant(&self, capsule_id: String, tenant_id: String) -> CapabilityGrant {
        let mut grant = CapabilityGrant::minimal(capsule_id, tenant_id, "manifest".to_string());

        // Add network capabilities based on allowed_hosts
        if !self.allowed_hosts.is_empty() {
            let allowed_hosts = self.allowed_hosts.clone();
            // For now, allow all ports (in a real implementation, you'd want to be more specific)
            let allowed_ports = vec![];
            grant = grant.with_capability(Capability::Network {
                allowed_hosts,
                allowed_ports,
            });
        }

        // Add filesystem capabilities based on allowed_paths
        if !self.allowed_paths.is_empty() {
            let read_paths: Vec<String> = self.allowed_paths.keys().cloned().collect();
            let write_paths: Vec<String> = self.allowed_paths.values().cloned().collect();

            if !read_paths.is_empty() {
                grant = grant.with_capability(Capability::FilesystemRead { paths: read_paths });
            }

            if !write_paths.is_empty() {
                grant = grant.with_capability(Capability::FilesystemWrite { paths: write_paths });
            }
        }

        // Add environment variable capabilities
        if !self.allowed_env.is_empty() {
            grant = grant.with_capability(Capability::Environment {
                allowed_vars: self.allowed_env.clone(),
            });
        }

        // Add HTTP client capability if hosts are allowed
        if !self.allowed_hosts.is_empty() {
            grant = grant.with_capability(Capability::HttpClient {
                allowed_hosts: self.allowed_hosts.clone(),
                max_requests_per_minute: 100, // Default rate limit
            });
        }

        // Add clock and random capabilities (basic requirements)
        grant = grant.with_capability(Capability::Clock);
        grant = grant.with_capability(Capability::Random);
        grant = grant.with_capability(Capability::Stdio);

        grant
    }

    /// Validate the manifest
    pub fn validate(&self) -> Result<(), String> {
        // Validate name
        if self.name.is_empty() {
            return Err("name cannot be empty".to_string());
        }

        // Validate version
        if self.version.is_empty() {
            return Err("version cannot be empty".to_string());
        }

        // Validate module source
        match &self.module {
            ModuleSource::File { path } => {
                if path.is_empty() {
                    return Err("module file path cannot be empty".to_string());
                }
            }
            ModuleSource::Url { url } => {
                if url.is_empty() {
                    return Err("module URL cannot be empty".to_string());
                }
                // Validate URL format
                Url::parse(url).map_err(|e| format!("invalid module URL: {}", e))?;
            }
            ModuleSource::Registry { name, version } => {
                if name.is_empty() {
                    return Err("registry module name cannot be empty".to_string());
                }
                if version.is_empty() {
                    return Err("registry module version cannot be empty".to_string());
                }
            }
            ModuleSource::Inline { data } => {
                if data.is_empty() {
                    return Err("inline module data cannot be empty".to_string());
                }
            }
        }

        // Validate allowed hosts
        for host in &self.allowed_hosts {
            if host.is_empty() {
                return Err("allowed_hosts cannot contain empty strings".to_string());
            }
            // Validate host format
            if host != "*" {
                // Simple validation - in a real implementation, you'd want more thorough validation
                if !host.contains('.') && host != "localhost" && !host.starts_with("127.") {
                    return Err(format!("invalid host format: {}", host));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_creation() {
        let manifest = CapsuleManifest::new(
            "test-capsule".to_string(),
            "1.0.0".to_string(),
            ModuleSource::File {
                path: "/path/to/module.wasm".to_string(),
            },
        );

        assert_eq!(manifest.name, "test-capsule");
        assert_eq!(manifest.version, "1.0.0");
        assert!(matches!(manifest.module, ModuleSource::File { .. }));
    }

    #[test]
    fn test_manifest_validation() {
        let manifest = CapsuleManifest::new(
            "test-capsule".to_string(),
            "1.0.0".to_string(),
            ModuleSource::File {
                path: "/path/to/module.wasm".to_string(),
            },
        );

        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_manifest_validation_fails_empty_name() {
        let mut manifest = CapsuleManifest::new(
            "".to_string(),
            "1.0.0".to_string(),
            ModuleSource::File {
                path: "/path/to/module.wasm".to_string(),
            },
        );

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_manifest_to_capability_grant() {
        let mut manifest = CapsuleManifest::new(
            "test-capsule".to_string(),
            "1.0.0".to_string(),
            ModuleSource::File {
                path: "/path/to/module.wasm".to_string(),
            },
        );

        manifest.allowed_hosts = vec!["https://api.example.com".to_string()];
        manifest.allowed_paths = vec![("/tmp".to_string(), "/tmp".to_string())]
            .into_iter()
            .collect();
        manifest.allowed_env = vec!["HOME".to_string()];

        let grant = manifest.to_capability_grant("test-id".to_string(), "test-tenant".to_string());

        assert!(grant.has_capability_type("network"));
        assert!(grant.has_capability_type("filesystem:read"));
        assert!(grant.has_capability_type("filesystem:write"));
        assert!(grant.has_capability_type("environment"));
        assert!(grant.has_capability_type("http_client"));
        assert!(grant.has_capability_type("clock"));
        assert!(grant.has_capability_type("random"));
        assert!(grant.has_capability_type("stdio"));
    }
}
