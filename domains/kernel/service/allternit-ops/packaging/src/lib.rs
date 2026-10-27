use allternit_context_router::ContextRouter;
use allternit_history::{HistoryError, HistoryLedger};
use allternit_memory::MemoryFabric;
use allternit_messaging::{EventEnvelope, MessagingSystem};
use allternit_policy::{PolicyEffect, PolicyEngine, PolicyRequest};
use allternit_providers::ProviderRouter;
use allternit_runtime_core::SessionManager;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    pub dependencies: Vec<PackageDependency>,
    pub components: Vec<PackageComponent>,
    pub configuration: PackageConfiguration,
    pub security_profile: SecurityProfile,
    pub deployment_targets: Vec<DeploymentTarget>,
    pub created_at: u64,
    pub updated_at: u64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDependency {
    pub package_id: String,
    pub version_constraint: String, // e.g., "^1.0.0", ">=1.0.0 <2.0.0"
    pub optional: bool,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PackageComponent {
    Skill {
        skill_id: String,
    },
    Workflow {
        workflow_id: String,
    },
    Provider {
        provider_id: String,
    },
    Embodiment {
        embodiment_id: String,
    },
    Custom {
        component_type: String,
        reference: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfiguration {
    pub environment_variables: HashMap<String, String>,
    pub resource_limits: ResourceLimits,
    pub security_settings: SecuritySettings,
    pub network_config: NetworkConfiguration,
    pub storage_config: StorageConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu: String,    // e.g., "500m" for 0.5 cores
    pub memory: String, // e.g., "512Mi" for 512 MiB
    pub disk: String,
    pub network: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
    pub read_only_root_filesystem: bool,
    pub allowed_capabilities: Vec<String>,
    pub dropped_capabilities: Vec<String>,
    pub privileged: bool,
    pub allow_privilege_escalation: bool,
    pub seccomp_profile: Option<String>,
    pub app_armor_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfiguration {
    pub ports: Vec<PortConfiguration>,
    pub dns_policy: DnsPolicy,
    pub host_network: bool,
    pub allowed_outbound_hosts: Vec<String>,
    pub allowed_inbound_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortConfiguration {
    pub port: u16,
    pub protocol: Protocol,
    pub target_port: Option<u16>,
    pub node_port: Option<u16>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Protocol {
    TCP,
    UDP,
    SCTP,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DnsPolicy {
    ClusterFirst,
    Default,
    None,
    ClusterFirstWithHostNet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfiguration {
    pub volumes: Vec<VolumeConfiguration>,
    pub persistent_volumes: Vec<PersistentVolumeConfiguration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeConfiguration {
    pub name: String,
    pub mount_path: String,
    pub read_only: bool,
    pub volume_type: VolumeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VolumeType {
    EmptyDir,
    ConfigMap { config_map_name: String },
    Secret { secret_name: String },
    HostPath { path: String },
    PersistentVolumeClaim { claim_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVolumeConfiguration {
    pub name: String,
    pub size: String,
    pub storage_class: Option<String>,
    pub access_modes: Vec<AccessMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessMode {
    ReadWriteOnce,
    ReadOnlyMany,
    ReadWriteMany,
    ReadWriteOncePod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityProfile {
    pub sensitivity_tier: u8, // 0-4 corresponding to T0-T4
    pub compliance_requirements: Vec<ComplianceRequirement>,
    pub audit_level: AuditLevel,
    pub encryption_required: bool,
    pub network_isolation: NetworkIsolation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComplianceRequirement {
    SOC2,
    HIPAA,
    GDPR,
    PCI_DSS,
    ISO27001,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditLevel {
    None,
    Basic,
    Standard,
    Enhanced,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkIsolation {
    None,
    Namespace,
    Node,
    Physical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentTarget {
    pub target_id: String,
    pub target_type: DeploymentTargetType,
    pub region: String,
    pub availability_zone: Option<String>,
    pub resource_requirements: ResourceRequirements,
    pub security_requirements: SecurityRequirements,
    pub network_requirements: NetworkRequirements,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeploymentTargetType {
    Kubernetes,
    Docker,
    BareMetal,
    VM,
    Edge,
    Cloud,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequirements {
    pub min_cpu: String,
    pub max_cpu: String,
    pub min_memory: String,
    pub max_memory: String,
    pub min_storage: String,
    pub max_storage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRequirements {
    pub min_bandwidth: String,
    pub max_bandwidth: String,
    pub allowed_endpoints: Vec<String>,
    pub security_level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityRequirements {
    pub min_sensitivity_tier: u8,
    pub compliance_requirements: Vec<ComplianceRequirement>,
    pub encryption_required: bool,
    pub network_isolation: NetworkIsolation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInstance {
    pub instance_id: String,
    pub package_id: String,
    pub version: String,
    pub deployment_target: DeploymentTarget,
    pub status: PackageInstanceStatus,
    pub configuration: PackageConfiguration,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_heartbeat: Option<u64>,
    pub health_status: HealthStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PackageInstanceStatus {
    Pending,
    Deploying,
    Running,
    Stopped,
    Failed,
    Terminated,
    Updating,
    Scaling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Degraded,
    Unknown,
}

#[derive(Debug, thiserror::Error)]
pub enum PackagingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] HistoryError),
    #[error("SQLX error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Policy error: {0}")]
    Policy(#[from] allternit_policy::PolicyError),
    #[error("Package not found: {0}")]
    PackageNotFound(String),
    #[error("Package already exists: {0}")]
    PackageAlreadyExists(String),
    #[error("Invalid package: {0}")]
    InvalidPackage(String),
    #[error("Deployment failed: {0}")]
    DeploymentFailed(String),
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("Dependency resolution failed: {0}")]
    DependencyResolutionFailed(String),
    #[error("Security violation: {0}")]
    SecurityViolation(String),
}

// Storage trait for package configurations
#[async_trait::async_trait]
pub trait PackageStorage: Send + Sync {
    async fn store_package(&self, config: &PackageConfig) -> Result<(), PackagingError>;
    async fn get_package(&self, package_id: &str) -> Result<Option<PackageConfig>, PackagingError>;
    async fn update_package(&self, config: &PackageConfig) -> Result<(), PackagingError>;
    async fn delete_package(&self, package_id: &str) -> Result<(), PackagingError>;

    async fn store_instance(&self, instance: &PackageInstance) -> Result<(), PackagingError>;
    async fn get_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<PackageInstance>, PackagingError>;
    async fn update_instance(&self, instance: &PackageInstance) -> Result<(), PackagingError>;
    async fn delete_instance(&self, instance_id: &str) -> Result<(), PackagingError>;

    async fn get_package_stats(&self, package_id: &str) -> Result<PackageStats, PackagingError>;
    async fn update_package_stats(&self, stats: &PackageStats) -> Result<(), PackagingError>;
}

pub struct SqlitePackageStorage {
    pool: SqlitePool,
}

impl SqlitePackageStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, PackagingError> {
        // Create the packages table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS packages (
                package_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                description TEXT NOT NULL,
                author TEXT NOT NULL,
                license TEXT NOT NULL,
                tags TEXT NOT NULL,
                dependencies TEXT NOT NULL,
                components TEXT NOT NULL,
                configuration TEXT NOT NULL,
                security_profile TEXT NOT NULL,
                deployment_targets TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                is_active BOOLEAN NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        // Create the package_instances table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS package_instances (
                instance_id TEXT PRIMARY KEY,
                package_id TEXT NOT NULL,
                version TEXT NOT NULL,
                deployment_target TEXT NOT NULL,
                status TEXT NOT NULL,
                configuration TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_heartbeat INTEGER,
                health_status TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        // Create indexes for better query performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_packages_name ON packages(name)")
            .execute(&pool)
            .await
            .map_err(PackagingError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_packages_active ON packages(is_active)")
            .execute(&pool)
            .await
            .map_err(PackagingError::Sqlx)?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_instances_package ON package_instances(package_id)",
        )
        .execute(&pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_instances_status ON package_instances(status)")
            .execute(&pool)
            .await
            .map_err(PackagingError::Sqlx)?;

        Ok(SqlitePackageStorage { pool })
    }
}

#[async_trait::async_trait]
impl PackageStorage for SqlitePackageStorage {
    async fn store_package(&self, config: &PackageConfig) -> Result<(), PackagingError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tags_json = serde_json::to_string(&config.tags).map_err(PackagingError::Json)?;
        let dependencies_json =
            serde_json::to_string(&config.dependencies).map_err(PackagingError::Json)?;
        let components_json =
            serde_json::to_string(&config.components).map_err(PackagingError::Json)?;
        let configuration_json =
            serde_json::to_string(&config.configuration).map_err(PackagingError::Json)?;
        let security_profile_json =
            serde_json::to_string(&config.security_profile).map_err(PackagingError::Json)?;
        let deployment_targets_json =
            serde_json::to_string(&config.deployment_targets).map_err(PackagingError::Json)?;

        sqlx::query(
            "INSERT OR REPLACE INTO packages (
                package_id, name, version, description, author, license,
                tags, dependencies, components, configuration, security_profile,
                deployment_targets, created_at, updated_at, is_active
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&config.package_id)
        .bind(&config.name)
        .bind(&config.version)
        .bind(&config.description)
        .bind(&config.author)
        .bind(&config.license)
        .bind(&tags_json)
        .bind(&dependencies_json)
        .bind(&components_json)
        .bind(&configuration_json)
        .bind(&security_profile_json)
        .bind(&deployment_targets_json)
        .bind(now as i64)
        .bind(now as i64)
        .bind(config.is_active)
        .execute(&self.pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        Ok(())
    }

    async fn get_package(&self, package_id: &str) -> Result<Option<PackageConfig>, PackagingError> {
        let row = sqlx::query(
            "SELECT package_id, name, version, description, author, license, tags, 
             dependencies, components, configuration, security_profile, deployment_targets, 
             created_at, updated_at, is_active
             FROM packages WHERE package_id = ?",
        )
        .bind(package_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        if let Some(row) = row {
            let tags: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("tags")).map_err(PackagingError::Json)?;
            let dependencies: Vec<PackageDependency> =
                serde_json::from_str(row.get::<&str, _>("dependencies"))
                    .map_err(PackagingError::Json)?;
            let components: Vec<PackageComponent> =
                serde_json::from_str(row.get::<&str, _>("components"))
                    .map_err(PackagingError::Json)?;
            let configuration: PackageConfiguration =
                serde_json::from_str(row.get::<&str, _>("configuration"))
                    .map_err(PackagingError::Json)?;
            let security_profile: SecurityProfile =
                serde_json::from_str(row.get::<&str, _>("security_profile"))
                    .map_err(PackagingError::Json)?;
            let deployment_targets: Vec<DeploymentTarget> =
                serde_json::from_str(row.get::<&str, _>("deployment_targets"))
                    .map_err(PackagingError::Json)?;

            let config = PackageConfig {
                package_id: row.get("package_id"),
                name: row.get("name"),
                version: row.get("version"),
                description: row.get("description"),
                author: row.get("author"),
                license: row.get("license"),
                tags,
                dependencies,
                components,
                configuration,
                security_profile,
                deployment_targets,
                created_at: row.get::<i64, _>("created_at") as u64,
                updated_at: row.get::<i64, _>("updated_at") as u64,
                is_active: row.get("is_active"),
            };

            Ok(Some(config))
        } else {
            Ok(None)
        }
    }

    async fn update_package(&self, config: &PackageConfig) -> Result<(), PackagingError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tags_json = serde_json::to_string(&config.tags).map_err(PackagingError::Json)?;
        let dependencies_json =
            serde_json::to_string(&config.dependencies).map_err(PackagingError::Json)?;
        let components_json =
            serde_json::to_string(&config.components).map_err(PackagingError::Json)?;
        let configuration_json =
            serde_json::to_string(&config.configuration).map_err(PackagingError::Json)?;
        let security_profile_json =
            serde_json::to_string(&config.security_profile).map_err(PackagingError::Json)?;
        let deployment_targets_json =
            serde_json::to_string(&config.deployment_targets).map_err(PackagingError::Json)?;

        sqlx::query(
            "UPDATE packages SET
                name = ?, version = ?, description = ?, author = ?, license = ?,
                tags = ?, dependencies = ?, components = ?, configuration = ?, 
                security_profile = ?, deployment_targets = ?, updated_at = ?, is_active = ?
             WHERE package_id = ?",
        )
        .bind(&config.name)
        .bind(&config.version)
        .bind(&config.description)
        .bind(&config.author)
        .bind(&config.license)
        .bind(&tags_json)
        .bind(&dependencies_json)
        .bind(&components_json)
        .bind(&configuration_json)
        .bind(&security_profile_json)
        .bind(&deployment_targets_json)
        .bind(now as i64)
        .bind(config.is_active)
        .bind(&config.package_id)
        .execute(&self.pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        Ok(())
    }

    async fn delete_package(&self, package_id: &str) -> Result<(), PackagingError> {
        sqlx::query("DELETE FROM packages WHERE package_id = ?")
            .bind(package_id)
            .execute(&self.pool)
            .await
            .map_err(PackagingError::Sqlx)?;

        Ok(())
    }

    async fn store_instance(&self, instance: &PackageInstance) -> Result<(), PackagingError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let deployment_target_json =
            serde_json::to_string(&instance.deployment_target).map_err(PackagingError::Json)?;
        let configuration_json =
            serde_json::to_string(&instance.configuration).map_err(PackagingError::Json)?;
        let status_str = format!("{:?}", instance.status);
        let health_status_str = format!("{:?}", instance.health_status);

        sqlx::query(
            "INSERT OR REPLACE INTO package_instances (
                instance_id, package_id, version, deployment_target, status, 
                configuration, created_at, updated_at, last_heartbeat, health_status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&instance.instance_id)
        .bind(&instance.package_id)
        .bind(&instance.version)
        .bind(&deployment_target_json)
        .bind(&status_str)
        .bind(&configuration_json)
        .bind(now as i64)
        .bind(now as i64)
        .bind(instance.last_heartbeat.map(|t| t as i64))
        .bind(&health_status_str)
        .execute(&self.pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        Ok(())
    }

    async fn get_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<PackageInstance>, PackagingError> {
        let row = sqlx::query(
            "SELECT instance_id, package_id, version, deployment_target, status, 
             configuration, created_at, updated_at, last_heartbeat, health_status
             FROM package_instances WHERE instance_id = ?",
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        if let Some(row) = row {
            let deployment_target: DeploymentTarget =
                serde_json::from_str(row.get::<&str, _>("deployment_target"))
                    .map_err(PackagingError::Json)?;
            let configuration: PackageConfiguration =
                serde_json::from_str(row.get::<&str, _>("configuration"))
                    .map_err(PackagingError::Json)?;

            let status_str: String = row.get("status");
            let status = match status_str.as_str() {
                "Pending" => PackageInstanceStatus::Pending,
                "Deploying" => PackageInstanceStatus::Deploying,
                "Running" => PackageInstanceStatus::Running,
                "Stopped" => PackageInstanceStatus::Stopped,
                "Failed" => PackageInstanceStatus::Failed,
                "Terminated" => PackageInstanceStatus::Terminated,
                "Updating" => PackageInstanceStatus::Updating,
                "Scaling" => PackageInstanceStatus::Scaling,
                _ => {
                    return Err(PackagingError::DeploymentFailed(
                        "Invalid instance status".to_string(),
                    ))
                }
            };

            let health_status_str: String = row.get("health_status");
            let health_status = match health_status_str.as_str() {
                "Healthy" => HealthStatus::Healthy,
                "Unhealthy" => HealthStatus::Unhealthy,
                "Degraded" => HealthStatus::Degraded,
                "Unknown" => HealthStatus::Unknown,
                _ => {
                    return Err(PackagingError::DeploymentFailed(
                        "Invalid health status".to_string(),
                    ))
                }
            };

            let instance = PackageInstance {
                instance_id: row.get("instance_id"),
                package_id: row.get("package_id"),
                version: row.get("version"),
                deployment_target,
                status,
                configuration,
                created_at: row.get::<i64, _>("created_at") as u64,
                updated_at: row.get::<i64, _>("updated_at") as u64,
                last_heartbeat: row
                    .get::<Option<i64>, _>("last_heartbeat")
                    .map(|t| t as u64),
                health_status,
            };

            Ok(Some(instance))
        } else {
            Ok(None)
        }
    }

    async fn update_instance(&self, instance: &PackageInstance) -> Result<(), PackagingError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let deployment_target_json =
            serde_json::to_string(&instance.deployment_target).map_err(PackagingError::Json)?;
        let configuration_json =
            serde_json::to_string(&instance.configuration).map_err(PackagingError::Json)?;
        let status_str = format!("{:?}", instance.status);
        let health_status_str = format!("{:?}", instance.health_status);

        sqlx::query(
            "UPDATE package_instances SET
                version = ?, deployment_target = ?, status = ?, configuration = ?, 
                updated_at = ?, last_heartbeat = ?, health_status = ?
             WHERE instance_id = ?",
        )
        .bind(&instance.version)
        .bind(&deployment_target_json)
        .bind(&status_str)
        .bind(&configuration_json)
        .bind(now as i64)
        .bind(instance.last_heartbeat.map(|t| t as i64))
        .bind(&health_status_str)
        .bind(&instance.instance_id)
        .execute(&self.pool)
        .await
        .map_err(PackagingError::Sqlx)?;

        Ok(())
    }

    async fn delete_instance(&self, instance_id: &str) -> Result<(), PackagingError> {
        sqlx::query("DELETE FROM package_instances WHERE instance_id = ?")
            .bind(instance_id)
            .execute(&self.pool)
            .await
            .map_err(PackagingError::Sqlx)?;

        Ok(())
    }

    async fn get_package_stats(&self, package_id: &str) -> Result<PackageStats, PackagingError> {
        // In a real implementation, this would fetch from a stats table
        // For now, return default stats
        Ok(PackageStats {
            package_id: package_id.to_string(),
            total_deployments: 0,
            total_instances: 0,
            active_instances: 0,
            avg_deployment_time: 0.0,
            success_rate: 1.0, // Initially assume 100% success
            last_deployment: None,
            error_count: 0,
            avg_resource_usage: ResourceUsage::default(),
        })
    }

    async fn update_package_stats(&self, stats: &PackageStats) -> Result<(), PackagingError> {
        // In a real implementation, this would update a stats table
        // For now, we'll just return Ok
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageStats {
    pub package_id: String,
    pub total_deployments: u64,
    pub total_instances: u64,
    pub active_instances: u64,
    pub avg_deployment_time: f64, // in seconds
    pub success_rate: f64,
    pub last_deployment: Option<u64>,
    pub error_count: u64,
    pub avg_resource_usage: ResourceUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceUsage {
    pub avg_cpu: f64,     // percentage
    pub avg_memory: f64,  // percentage
    pub avg_disk: f64,    // percentage
    pub avg_network: f64, // percentage
    pub peak_cpu: f64,
    pub peak_memory: f64,
    pub peak_disk: f64,
    pub peak_network: f64,
}

pub struct PackageManager {
    packages: Arc<RwLock<HashMap<String, PackageConfig>>>,
    instances: Arc<RwLock<HashMap<String, PackageInstance>>>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    context_router: Arc<ContextRouter>,
    memory_fabric: Arc<MemoryFabric>,
    provider_router: Arc<ProviderRouter>,
    session_manager: Arc<SessionManager>,
    storage: Arc<dyn PackageStorage>,
}

impl PackageManager {
    pub async fn new_with_storage(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        context_router: Arc<ContextRouter>,
        memory_fabric: Arc<MemoryFabric>,
        provider_router: Arc<ProviderRouter>,
        session_manager: Arc<SessionManager>,
        pool: SqlitePool,
    ) -> Result<Self, PackagingError> {
        let storage = Arc::new(SqlitePackageStorage::new(pool).await?);

        // Load existing packages and instances from storage
        let packages_map = HashMap::new();
        let instances_map = HashMap::new();

        // In a real implementation, we would load from storage
        // For now, we'll initialize empty maps

        Ok(PackageManager {
            packages: Arc::new(RwLock::new(packages_map)),
            instances: Arc::new(RwLock::new(instances_map)),
            history_ledger,
            messaging_system,
            policy_engine,
            context_router,
            memory_fabric,
            provider_router,
            session_manager,
            storage,
        })
    }

    pub async fn create_package(
        &self,
        mut config: PackageConfig,
    ) -> Result<String, PackagingError> {
        // Validate package config
        self.validate_package_config(&config).await?;

        // Check if package already exists
        {
            let packages = self.packages.read().await;
            if packages.contains_key(&config.package_id) {
                return Err(PackagingError::PackageAlreadyExists(config.package_id));
            }
        }

        // Validate dependencies
        self.validate_dependencies(&config.dependencies).await?;

        // Validate security profile
        self.validate_security_profile(&config.security_profile)
            .await?;

        // Set creation timestamp
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        config.created_at = now;
        config.updated_at = now;

        // Store in durable storage
        self.storage.store_package(&config).await?;

        // Update in-memory cache
        let mut packages = self.packages.write().await;
        packages.insert(config.package_id.clone(), config.clone());

        // Log the event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "PackageCreated".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Would be the actual creator in real implementation
            role: "package_manager".to_string(),
            timestamp: now,
            trace_id: None,
            payload: serde_json::json!({
                "package_id": config.package_id,
                "name": config.name,
                "version": config.version,
                "components": config.components,
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        // Emit event asynchronously
        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(config.package_id)
    }

    async fn validate_package_config(&self, config: &PackageConfig) -> Result<(), PackagingError> {
        // Validate package ID
        if config.package_id.is_empty() {
            return Err(PackagingError::InvalidPackage(
                "Package ID cannot be empty".to_string(),
            ));
        }

        // Validate name
        if config.name.is_empty() {
            return Err(PackagingError::InvalidPackage(
                "Package name cannot be empty".to_string(),
            ));
        }

        // Validate version (simple semver check)
        let version_parts: Vec<&str> = config.version.split('.').collect();
        if version_parts.len() != 3 {
            return Err(PackagingError::InvalidPackage(
                "Version must be in semver format (x.y.z)".to_string(),
            ));
        }

        // Validate author
        if config.author.is_empty() {
            return Err(PackagingError::InvalidPackage(
                "Author cannot be empty".to_string(),
            ));
        }

        // Validate license
        if config.license.is_empty() {
            return Err(PackagingError::InvalidPackage(
                "License cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    async fn validate_dependencies(
        &self,
        dependencies: &[PackageDependency],
    ) -> Result<(), PackagingError> {
        // In a real implementation, this would check if dependencies exist and are compatible
        // For now, just validate the structure
        for dep in dependencies {
            if dep.package_id.is_empty() {
                return Err(PackagingError::DependencyResolutionFailed(
                    "Dependency package ID cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn validate_security_profile(
        &self,
        profile: &SecurityProfile,
    ) -> Result<(), PackagingError> {
        // Validate sensitivity tier is within range
        if profile.sensitivity_tier > 4 {
            return Err(PackagingError::SecurityViolation(
                "Sensitivity tier must be between 0 and 4".to_string(),
            ));
        }

        // In a real implementation, this would validate compliance requirements
        // For now, just return Ok
        Ok(())
    }

    pub async fn deploy_package(
        &self,
        package_id: String,
        target: DeploymentTarget,
    ) -> Result<String, PackagingError> {
        // Get the package
        let package = {
            let packages = self.packages.read().await;
            packages
                .get(&package_id)
                .cloned()
                .ok_or_else(|| PackagingError::PackageNotFound(package_id.clone()))?
        };

        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: "system".to_string(), // In real implementation, this would be the deploying user/agent
            resource: format!("package_deployment:{}", package_id),
            action: "deploy".to_string(),
            context: serde_json::json!({
                "package_id": package_id,
                "target": &target,
                "security_profile": &package.security_profile,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for deployment
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(PackagingError::SecurityViolation(format!(
                "Policy denied package deployment: {}",
                policy_decision.reason
            )));
        }

        // Validate target requirements against package requirements
        self.validate_target_requirements(&package, &target).await?;

        // Create a package instance
        let instance_id = Uuid::new_v4().to_string();
        let instance = PackageInstance {
            instance_id: instance_id.clone(),
            package_id: package.package_id.clone(),
            version: package.version.clone(),
            deployment_target: target.clone(),
            status: PackageInstanceStatus::Pending,
            configuration: package.configuration.clone(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            last_heartbeat: None,
            health_status: HealthStatus::Unknown,
        };

        // Store the instance
        self.storage.store_instance(&instance).await?;
        {
            let mut instances = self.instances.write().await;
            instances.insert(instance_id.clone(), instance);
        }

        // In a real implementation, this would trigger the actual deployment process
        // For now, we'll simulate a successful deployment
        self.simulate_deployment(&instance_id).await?;

        // Log the deployment event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "PackageDeployed".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Would be the actual deployer in real implementation
            role: "package_manager".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "instance_id": instance_id,
                "package_id": package_id,
                "target": target,
                "status": "success",
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        // Emit event asynchronously
        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(instance_id)
    }

    async fn validate_target_requirements(
        &self,
        package: &PackageConfig,
        target: &DeploymentTarget,
    ) -> Result<(), PackagingError> {
        // Check if target meets resource requirements
        if target.resource_requirements.min_cpu > package.configuration.resource_limits.cpu {
            return Err(PackagingError::DeploymentFailed(
                "Target does not meet minimum CPU requirements".to_string(),
            ));
        }

        if target.resource_requirements.min_memory > package.configuration.resource_limits.memory {
            return Err(PackagingError::DeploymentFailed(
                "Target does not meet minimum memory requirements".to_string(),
            ));
        }

        // Check if target meets security requirements
        if target.security_requirements.min_sensitivity_tier
            > package.security_profile.sensitivity_tier
        {
            return Err(PackagingError::SecurityViolation(
                "Target does not meet minimum security requirements".to_string(),
            ));
        }

        Ok(())
    }

    async fn simulate_deployment(&self, instance_id: &str) -> Result<(), PackagingError> {
        // In a real implementation, this would perform the actual deployment
        // For now, we'll simulate the process by updating the instance status

        // Get the current instance
        let mut instance = {
            let instances = self.instances.read().await;
            instances.get(instance_id).cloned().ok_or_else(|| {
                PackagingError::DeploymentFailed(format!("Instance {} not found", instance_id))
            })?
        };

        // Update status to deploying
        instance.status = PackageInstanceStatus::Deploying;
        instance.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Store the updated instance
        self.storage.update_instance(&instance).await?;
        {
            let mut instances = self.instances.write().await;
            instances.insert(instance_id.to_string(), instance.clone());
        }

        // Simulate deployment time
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Update status to running
        instance.status = PackageInstanceStatus::Running;
        instance.health_status = HealthStatus::Healthy;
        instance.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        instance.last_heartbeat = Some(instance.updated_at);

        // Store the final instance
        self.storage.update_instance(&instance).await?;
        {
            let mut instances = self.instances.write().await;
            instances.insert(instance_id.to_string(), instance);
        }

        Ok(())
    }

    pub async fn get_package(
        &self,
        package_id: String,
    ) -> Result<Option<PackageConfig>, PackagingError> {
        // Try to get from cache first
        {
            let packages = self.packages.read().await;
            if let Some(package) = packages.get(&package_id) {
                return Ok(Some(package.clone()));
            }
        }

        // If not in cache, get from storage
        let package = self.storage.get_package(&package_id).await?;
        if let Some(package) = &package {
            // Update cache
            let mut packages = self.packages.write().await;
            packages.insert(package_id.clone(), package.clone());
        }

        Ok(package)
    }

    pub async fn get_package_stats(
        &self,
        package_id: String,
    ) -> Result<PackageStats, PackagingError> {
        self.storage.get_package_stats(&package_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_package_manager_basic_functionality() {
        // Create temporary database
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        // Create temporary history ledger
        let temp_path = format!("/tmp/test_packaging_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        // Create messaging system
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create policy engine
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let system_identity = allternit_policy::Identity {
            id: "system".to_string(),
            identity_type: allternit_policy::IdentityType::ServiceAccount,
            name: "System".to_string(),
            tenant_id: "system".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["system".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine
            .register_identity(system_identity)
            .await
            .unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        policy_engine.create_default_rules().await.unwrap();
        let deploy_rule = allternit_policy::PolicyRule {
            id: "rule_allow_deploy".to_string(),
            name: "Allow Deploy Operations".to_string(),
            description: "Allow package deployment in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["deploy".to_string()],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(deploy_rule).await.unwrap();

        // Create context router
        let context_router = Arc::new(allternit_context_router::ContextRouter::new(
            history_ledger.clone(),
            messaging_system.clone(),
            policy_engine.clone(),
            Arc::new(allternit_runtime_core::SessionManager::new(
                history_ledger.clone(),
                messaging_system.clone(),
            )),
        ));

        // Create memory fabric
        let memory_fabric = Arc::new(
            allternit_memory::MemoryFabric::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                pool.clone(),
                Arc::new(allternit_memory::v2::memory_policy::DefaultMemoryPolicy),
            )
            .await
            .unwrap(),
        );

        // Create provider router
        let provider_router = Arc::new(
            allternit_providers::ProviderRouter::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create session manager
        let session_manager = Arc::new(allternit_runtime_core::SessionManager::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create package manager
        let package_manager = Arc::new(
            PackageManager::new_with_storage(
                history_ledger,
                messaging_system,
                policy_engine,
                context_router,
                memory_fabric,
                provider_router,
                session_manager,
                pool,
            )
            .await
            .unwrap(),
        );

        // Create a test package
        let package_config = PackageConfig {
            package_id: "test-package-001".to_string(),
            name: "Test Package".to_string(),
            version: "1.0.0".to_string(),
            description: "A test package for Allternit".to_string(),
            author: "Test Author".to_string(),
            license: "MIT".to_string(),
            tags: vec!["test".to_string(), "allternit".to_string()],
            dependencies: vec![],
            components: vec![
                PackageComponent::Skill {
                    skill_id: "test-skill".to_string(),
                },
                PackageComponent::Workflow {
                    workflow_id: "test-workflow".to_string(),
                },
            ],
            configuration: PackageConfiguration {
                environment_variables: HashMap::new(),
                resource_limits: ResourceLimits {
                    cpu: "500m".to_string(),
                    memory: "512Mi".to_string(),
                    disk: "1Gi".to_string(),
                    network: "100Mbps".to_string(),
                },
                security_settings: SecuritySettings {
                    run_as_user: Some(1000),
                    run_as_group: Some(1000),
                    read_only_root_filesystem: true,
                    allowed_capabilities: vec!["NET_BIND_SERVICE".to_string()],
                    dropped_capabilities: vec!["ALL".to_string()],
                    privileged: false,
                    allow_privilege_escalation: false,
                    seccomp_profile: Some("runtime/default".to_string()),
                    app_armor_profile: None,
                },
                network_config: NetworkConfiguration {
                    ports: vec![PortConfiguration {
                        port: 8080,
                        protocol: Protocol::TCP,
                        target_port: Some(80),
                        node_port: None,
                        name: Some("http".to_string()),
                    }],
                    dns_policy: DnsPolicy::ClusterFirst,
                    host_network: false,
                    allowed_outbound_hosts: vec!["api.example.com".to_string()],
                    allowed_inbound_hosts: vec![],
                },
                storage_config: StorageConfiguration {
                    volumes: vec![VolumeConfiguration {
                        name: "data".to_string(),
                        mount_path: "/data".to_string(),
                        read_only: false,
                        volume_type: VolumeType::EmptyDir,
                    }],
                    persistent_volumes: vec![],
                },
            },
            security_profile: SecurityProfile {
                sensitivity_tier: 2,
                compliance_requirements: vec![ComplianceRequirement::SOC2],
                audit_level: AuditLevel::Standard,
                encryption_required: true,
                network_isolation: NetworkIsolation::Namespace,
            },
            deployment_targets: vec![DeploymentTarget {
                target_id: "local-k8s".to_string(),
                target_type: DeploymentTargetType::Kubernetes,
                region: "local".to_string(),
                availability_zone: Some("local-1a".to_string()),
                resource_requirements: ResourceRequirements {
                    min_cpu: "250m".to_string(),
                    max_cpu: "1000m".to_string(),
                    min_memory: "256Mi".to_string(),
                    max_memory: "1Gi".to_string(),
                    min_storage: "1Gi".to_string(),
                    max_storage: "10Gi".to_string(),
                },
                security_requirements: SecurityRequirements {
                    min_sensitivity_tier: 0,
                    compliance_requirements: vec![ComplianceRequirement::SOC2],
                    encryption_required: true,
                    network_isolation: NetworkIsolation::Namespace,
                },
                network_requirements: NetworkRequirements {
                    min_bandwidth: "0Mbps".to_string(),
                    max_bandwidth: "100Mbps".to_string(),
                    allowed_endpoints: vec!["api.example.com".to_string()],
                    security_level: 1,
                },
            }],
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            is_active: true,
        };

        // Create the package
        let package_id = package_manager
            .create_package(package_config)
            .await
            .unwrap();
        assert_eq!(package_id, "test-package-001");

        // Deploy the package
        let deployment_target = DeploymentTarget {
            target_id: "local-k8s".to_string(),
            target_type: DeploymentTargetType::Kubernetes,
            region: "local".to_string(),
            availability_zone: Some("local-1a".to_string()),
            resource_requirements: ResourceRequirements {
                min_cpu: "250m".to_string(),
                max_cpu: "1000m".to_string(),
                min_memory: "256Mi".to_string(),
                max_memory: "1Gi".to_string(),
                min_storage: "1Gi".to_string(),
                max_storage: "10Gi".to_string(),
            },
            security_requirements: SecurityRequirements {
                min_sensitivity_tier: 0,
                compliance_requirements: vec![ComplianceRequirement::SOC2],
                encryption_required: true,
                network_isolation: NetworkIsolation::Namespace,
            },
            network_requirements: NetworkRequirements {
                min_bandwidth: "0Mbps".to_string(),
                max_bandwidth: "100Mbps".to_string(),
                allowed_endpoints: vec!["api.example.com".to_string()],
                security_level: 1,
            },
        };

        let instance_id = package_manager
            .deploy_package("test-package-001".to_string(), deployment_target)
            .await
            .unwrap();
        assert!(!instance_id.is_empty());

        // Verify the package was created
        let packages = package_manager.packages.read().await;
        assert_eq!(packages.len(), 1);
        assert!(packages.contains_key("test-package-001"));
        drop(packages);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
