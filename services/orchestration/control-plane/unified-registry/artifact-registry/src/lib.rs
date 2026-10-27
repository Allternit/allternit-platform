use allternit_context_router::{ContextBundle, ContextRouter};
use allternit_embodiment::EmbodimentControlPlane;
use allternit_evals::EvaluationEngine;
use allternit_history::{HistoryError, HistoryLedger};
use allternit_memory::MemoryFabric;
use allternit_messaging::{EventEnvelope, MessagingSystem};
use allternit_packaging::PackageManager;
use allternit_policy::{PolicyEffect, PolicyEngine, PolicyRequest};
use allternit_providers::ProviderRouter;
use allternit_runtime_core::SessionManager;
use allternit_skills::SkillRegistry;
use allternit_workflows::WorkflowEngine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRegistryConfig {
    pub registry_id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub storage_config: StorageConfig,
    pub security_profile: SecurityProfile,
    pub network_config: NetworkConfiguration,
    pub created_at: u64,
    pub updated_at: u64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub storage_type: StorageType,
    pub storage_path: String,
    pub encryption_enabled: bool,
    pub compression_enabled: bool,
    pub retention_policy: RetentionPolicy,
    pub backup_config: BackupConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    LocalFileSystem,
    S3,
    IPFS,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub retention_period_days: u32,
    pub auto_delete: bool,
    pub backup_before_delete: bool,
    pub compliance_hold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    pub enabled: bool,
    pub backup_frequency_hours: u32,
    pub backup_location: String,
    pub encryption_enabled: bool,
    pub retention_count: u32,
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
pub struct NetworkConfiguration {
    pub allowed_origins: Vec<String>,
    pub cors_enabled: bool,
    pub tls_required: bool,
    pub rate_limiting: Option<RateLimitingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitingConfig {
    pub requests_per_minute: u32,
    pub burst_size: u32,
    pub per_ip_limit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub artifact_id: String,
    pub name: String,
    pub version: String,
    pub artifact_type: ArtifactType,
    pub description: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    pub dependencies: Vec<ArtifactDependency>,
    pub content_hash: String,      // SHA256 of the artifact content
    pub signature: Option<String>, // Digital signature
    pub publisher: PublisherInfo,
    pub created_at: u64,
    pub updated_at: u64,
    pub published_at: Option<u64>,
    pub deprecated_at: Option<u64>,
    pub deprecation_reason: Option<String>,
    pub download_count: u64,
    pub rating: Option<f64>, // Average rating 0.0-5.0
    pub review_status: ReviewStatus,
    pub security_scan_results: Option<SecurityScanResults>,
    pub license_compliance: LicenseCompliance,
    pub trust_score: f64, // 0.0-1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    Skill,
    Persona,
    Workflow,
    PolicyBundle,
    Package,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactDependency {
    pub artifact_id: String,
    pub version_constraint: String, // e.g., "^1.0.0", ">=1.0.0 <2.0.0"
    pub optional: bool,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherInfo {
    pub publisher_id: String,
    pub name: String,
    pub email: String,
    pub organization: Option<String>,
    pub website: Option<String>,
    pub public_key: String,    // Public key for signature verification
    pub reputation_score: f64, // 0.0-1.0
    pub verification_status: VerificationStatus,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerificationStatus {
    Unverified,
    Verified,
    Trusted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
    Suspended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityScanResults {
    pub scan_id: String,
    pub scanner: String,
    pub scan_date: u64,
    pub vulnerabilities: Vec<Vulnerability>,
    pub malware_detected: bool,
    pub security_score: f64, // 0.0-1.0
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vulnerability {
    pub vulnerability_id: String,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    pub cvss_score: Option<f64>,
    pub remediation: Option<String>,
    pub affected_components: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseCompliance {
    pub license_approved: bool,
    pub license_text: String,
    pub attribution_required: bool,
    pub distribution_allowed: bool,
    pub modification_allowed: bool,
    pub patent_grant: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    pub request_id: String,
    pub artifact: ArtifactMetadata,
    pub content: Vec<u8>,           // Raw artifact content
    pub signature: String,          // Publisher signature
    pub verification_token: String, // For publisher verification
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishResponse {
    pub request_id: String,
    pub artifact_id: String,
    pub version: String,
    pub status: PublishStatus,
    pub message: String,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PublishStatus {
    Success,
    ValidationFailed,
    SignatureVerificationFailed,
    SecurityScanFailed,
    LicenseComplianceFailed,
    DuplicateArtifact,
    QuotaExceeded,
    AccessDenied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactQuery {
    pub query_id: String,
    pub filters: ArtifactFilters,
    pub sort_by: SortField,
    pub sort_direction: SortDirection,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub include_deprecated: bool,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactFilters {
    pub name: Option<String>,
    pub version: Option<String>,
    pub artifact_type: Option<ArtifactType>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub publisher_id: Option<String>,
    pub min_trust_score: Option<f64>,
    pub min_rating: Option<f64>,
    pub license_approved: Option<bool>,
    pub review_status: Option<ReviewStatus>,
    pub created_after: Option<u64>,
    pub created_before: Option<u64>,
    pub updated_after: Option<u64>,
    pub updated_before: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SortField {
    Name,
    CreatedAt,
    UpdatedAt,
    Rating,
    DownloadCount,
    TrustScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactQueryResponse {
    pub query_id: String,
    pub artifacts: Vec<ArtifactMetadata>,
    pub total_count: u64,
    pub returned_count: u32,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub request_id: String,
    pub artifact_id: String,
    pub version: Option<String>,
    pub tenant_id: String,
    pub requesting_agent: String,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResponse {
    pub request_id: String,
    pub artifact_id: String,
    pub version: String,
    pub content: Vec<u8>,
    pub content_hash: String,
    pub download_url: Option<String>,
    pub expires_at: Option<u64>,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyResolutionRequest {
    pub request_id: String,
    pub artifact_id: String,
    pub version: String,
    pub target_environment: Environment,
    pub security_requirements: Vec<SecurityRequirement>,
    pub license_requirements: Vec<LicenseRequirement>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub os: String,
    pub arch: String,
    pub runtime: String,
    pub runtime_version: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityRequirement {
    MaxSeverity(Severity),
    NoKnownVulnerabilities,
    SignedArtifactsOnly,
    TrustedPublishersOnly,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LicenseRequirement {
    PermissiveOnly,
    NoCopyleft,
    NoPatentRestrictions,
    ApprovedLicensesOnly(Vec<String>),
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyResolutionResponse {
    pub request_id: String,
    pub resolved_artifacts: Vec<ArtifactMetadata>,
    pub conflicts: Vec<DependencyConflict>,
    pub security_warnings: Vec<SecurityWarning>,
    pub license_warnings: Vec<LicenseWarning>,
    pub timestamp: u64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyConflict {
    pub artifact_id: String,
    pub conflicting_dependency: String,
    pub conflict_reason: String,
    pub suggested_resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityWarning {
    pub artifact_id: String,
    pub severity: Severity,
    pub warning_type: SecurityWarningType,
    pub description: String,
    pub mitigation_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityWarningType {
    KnownVulnerability,
    UntrustedPublisher,
    UnsignedArtifact,
    ExpiredCertificate,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseWarning {
    pub artifact_id: String,
    pub license_issue: LicenseIssue,
    pub description: String,
    pub compliance_risk: ComplianceRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LicenseIssue {
    UnapprovedLicense,
    AttributionRequired,
    DistributionRestriction,
    ModificationRestriction,
    PatentClause,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComplianceRisk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactRegistryError {
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
    #[error("Artifact not found: {0}")]
    ArtifactNotFound(String),
    #[error("Publisher not found: {0}")]
    PublisherNotFound(String),
    #[error("Invalid artifact: {0}")]
    InvalidArtifact(String),
    #[error("Signature verification failed: {0}")]
    SignatureVerificationFailed(String),
    #[error("Security scan failed: {0}")]
    SecurityScanFailed(String),
    #[error("License compliance failed: {0}")]
    LicenseComplianceFailed(String),
    #[error("Dependency resolution failed: {0}")]
    DependencyResolutionFailed(String),
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("Access denied: {0}")]
    AccessDenied(String),
}

// Storage trait for artifact metadata
#[async_trait::async_trait]
pub trait ArtifactStorage: Send + Sync {
    async fn store_artifact_metadata(
        &self,
        metadata: &ArtifactMetadata,
    ) -> Result<(), ArtifactRegistryError>;
    async fn get_artifact_metadata(
        &self,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<Option<ArtifactMetadata>, ArtifactRegistryError>;
    async fn update_artifact_metadata(
        &self,
        metadata: &ArtifactMetadata,
    ) -> Result<(), ArtifactRegistryError>;
    async fn delete_artifact_metadata(
        &self,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<(), ArtifactRegistryError>;

    async fn store_publisher_info(
        &self,
        publisher: &PublisherInfo,
    ) -> Result<(), ArtifactRegistryError>;
    async fn get_publisher_info(
        &self,
        publisher_id: &str,
    ) -> Result<Option<PublisherInfo>, ArtifactRegistryError>;
    async fn update_publisher_info(
        &self,
        publisher: &PublisherInfo,
    ) -> Result<(), ArtifactRegistryError>;

    async fn search_artifacts(
        &self,
        filters: &ArtifactFilters,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ArtifactMetadata>, ArtifactRegistryError>;
    async fn get_artifact_stats(
        &self,
        artifact_id: &str,
    ) -> Result<ArtifactStats, ArtifactRegistryError>;
    async fn update_artifact_stats(
        &self,
        stats: &ArtifactStats,
    ) -> Result<(), ArtifactRegistryError>;
}

pub struct SqliteArtifactStorage {
    pool: SqlitePool,
}

impl SqliteArtifactStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, ArtifactRegistryError> {
        // Create the artifacts table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS artifacts (
                artifact_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                artifact_type TEXT NOT NULL,
                description TEXT NOT NULL,
                author TEXT NOT NULL,
                license TEXT NOT NULL,
                tags TEXT NOT NULL,
                dependencies TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                signature TEXT,
                publisher_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                published_at INTEGER,
                deprecated_at INTEGER,
                deprecation_reason TEXT,
                download_count INTEGER NOT NULL DEFAULT 0,
                rating REAL,
                review_status TEXT NOT NULL,
                security_scan_results TEXT,
                license_compliance TEXT NOT NULL,
                trust_score REAL NOT NULL DEFAULT 0.0
            )",
        )
        .execute(&pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        // Create the publishers table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS publishers (
                publisher_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT NOT NULL,
                organization TEXT,
                website TEXT,
                public_key TEXT NOT NULL,
                reputation_score REAL NOT NULL DEFAULT 0.0,
                verification_status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        // Create indexes for better query performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_artifacts_name ON artifacts(name)")
            .execute(&pool)
            .await
            .map_err(ArtifactRegistryError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_artifacts_type ON artifacts(artifact_type)")
            .execute(&pool)
            .await
            .map_err(ArtifactRegistryError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_artifacts_author ON artifacts(author)")
            .execute(&pool)
            .await
            .map_err(ArtifactRegistryError::Sqlx)?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_artifacts_publisher ON artifacts(publisher_id)",
        )
        .execute(&pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_artifacts_created_at ON artifacts(created_at)")
            .execute(&pool)
            .await
            .map_err(ArtifactRegistryError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_publishers_email ON publishers(email)")
            .execute(&pool)
            .await
            .map_err(ArtifactRegistryError::Sqlx)?;

        Ok(SqliteArtifactStorage { pool })
    }
}

#[async_trait::async_trait]
impl ArtifactStorage for SqliteArtifactStorage {
    async fn store_artifact_metadata(
        &self,
        metadata: &ArtifactMetadata,
    ) -> Result<(), ArtifactRegistryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tags_json =
            serde_json::to_string(&metadata.tags).map_err(ArtifactRegistryError::Json)?;
        let dependencies_json =
            serde_json::to_string(&metadata.dependencies).map_err(ArtifactRegistryError::Json)?;
        let review_status_str = format!("{:?}", metadata.review_status);
        let security_scan_results_json = metadata
            .security_scan_results
            .as_ref()
            .map(|s| serde_json::to_string(s))
            .transpose()
            .map_err(ArtifactRegistryError::Json)?;
        let license_compliance_json = serde_json::to_string(&metadata.license_compliance)
            .map_err(ArtifactRegistryError::Json)?;

        sqlx::query(
            "INSERT OR REPLACE INTO artifacts (
                artifact_id, name, version, artifact_type, description, author, license,
                tags, dependencies, content_hash, signature, publisher_id,
                created_at, updated_at, published_at, deprecated_at, deprecation_reason,
                download_count, rating, review_status, security_scan_results, license_compliance, trust_score
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&metadata.artifact_id)
        .bind(&metadata.name)
        .bind(&metadata.version)
        .bind(format!("{:?}", metadata.artifact_type))
        .bind(&metadata.description)
        .bind(&metadata.author)
        .bind(&metadata.license)
        .bind(&tags_json)
        .bind(&dependencies_json)
        .bind(&metadata.content_hash)
        .bind(&metadata.signature)
        .bind(&metadata.publisher.publisher_id)
        .bind(now as i64)
        .bind(now as i64)
        .bind(metadata.published_at.map(|t| t as i64))
        .bind(metadata.deprecated_at.map(|t| t as i64))
        .bind(&metadata.deprecation_reason)
        .bind(metadata.download_count as i64)
        .bind(metadata.rating)
        .bind(&review_status_str)
        .bind(&security_scan_results_json)
        .bind(&license_compliance_json)
        .bind(metadata.trust_score)
        .execute(&self.pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        Ok(())
    }

    async fn get_artifact_metadata(
        &self,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<Option<ArtifactMetadata>, ArtifactRegistryError> {
        let query = if version.is_some() {
            "SELECT * FROM artifacts WHERE artifact_id = ? AND version = ?"
        } else {
            "SELECT * FROM artifacts WHERE artifact_id = ? ORDER BY created_at DESC LIMIT 1"
        };

        let row = if let Some(ver) = version {
            sqlx::query(query)
                .bind(artifact_id)
                .bind(ver)
                .fetch_optional(&self.pool)
                .await
                .map_err(ArtifactRegistryError::Sqlx)?
        } else {
            sqlx::query(query)
                .bind(artifact_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(ArtifactRegistryError::Sqlx)?
        };

        if let Some(row) = row {
            let tags: Vec<String> = serde_json::from_str(row.get::<&str, _>("tags"))
                .map_err(ArtifactRegistryError::Json)?;
            let dependencies: Vec<ArtifactDependency> =
                serde_json::from_str(row.get::<&str, _>("dependencies"))
                    .map_err(ArtifactRegistryError::Json)?;

            let review_status_str: String = row.get("review_status");
            let review_status = match review_status_str.as_str() {
                "Pending" => ReviewStatus::Pending,
                "Approved" => ReviewStatus::Approved,
                "Rejected" => ReviewStatus::Rejected,
                "Suspended" => ReviewStatus::Suspended,
                _ => {
                    return Err(ArtifactRegistryError::InvalidArtifact(
                        "Invalid review status".to_string(),
                    ))
                }
            };

            let security_scan_results: Option<SecurityScanResults> =
                if let Some(json_str) = row.get::<Option<&str>, _>("security_scan_results") {
                    Some(serde_json::from_str(json_str).map_err(ArtifactRegistryError::Json)?)
                } else {
                    None
                };

            let license_compliance: LicenseCompliance =
                serde_json::from_str(row.get::<&str, _>("license_compliance"))
                    .map_err(ArtifactRegistryError::Json)?;

            // Get publisher info
            let publisher_id: String = row.get("publisher_id");
            let publisher = self
                .get_publisher_info(&publisher_id)
                .await?
                .ok_or_else(|| ArtifactRegistryError::PublisherNotFound(publisher_id.clone()))?;

            let artifact_type_str: String = row.get("artifact_type");
            let artifact_type = match artifact_type_str.as_str() {
                "Skill" => ArtifactType::Skill,
                "Persona" => ArtifactType::Persona,
                "Workflow" => ArtifactType::Workflow,
                "PolicyBundle" => ArtifactType::PolicyBundle,
                "Package" => ArtifactType::Package,
                custom => ArtifactType::Custom(custom.to_string()),
            };

            let metadata = ArtifactMetadata {
                artifact_id: row.get("artifact_id"),
                name: row.get("name"),
                version: row.get("version"),
                artifact_type,
                description: row.get("description"),
                author: row.get("author"),
                license: row.get("license"),
                tags,
                dependencies,
                content_hash: row.get("content_hash"),
                signature: row.get("signature"),
                publisher,
                created_at: row.get::<i64, _>("created_at") as u64,
                updated_at: row.get::<i64, _>("updated_at") as u64,
                published_at: row.get::<Option<i64>, _>("published_at").map(|t| t as u64),
                deprecated_at: row.get::<Option<i64>, _>("deprecated_at").map(|t| t as u64),
                deprecation_reason: row.get("deprecation_reason"),
                download_count: row.get::<i64, _>("download_count") as u64,
                rating: row.get("rating"),
                review_status,
                security_scan_results,
                license_compliance,
                trust_score: row.get::<f64, _>("trust_score"),
            };

            Ok(Some(metadata))
        } else {
            Ok(None)
        }
    }

    async fn update_artifact_metadata(
        &self,
        metadata: &ArtifactMetadata,
    ) -> Result<(), ArtifactRegistryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tags_json =
            serde_json::to_string(&metadata.tags).map_err(ArtifactRegistryError::Json)?;
        let dependencies_json =
            serde_json::to_string(&metadata.dependencies).map_err(ArtifactRegistryError::Json)?;
        let review_status_str = format!("{:?}", metadata.review_status);
        let security_scan_results_json = metadata
            .security_scan_results
            .as_ref()
            .map(|s| serde_json::to_string(s))
            .transpose()
            .map_err(ArtifactRegistryError::Json)?;
        let license_compliance_json = serde_json::to_string(&metadata.license_compliance)
            .map_err(ArtifactRegistryError::Json)?;

        sqlx::query(
            "UPDATE artifacts SET
                name = ?, version = ?, artifact_type = ?, description = ?, author = ?, license = ?,
                tags = ?, dependencies = ?, content_hash = ?, signature = ?, publisher_id = ?,
                updated_at = ?, published_at = ?, deprecated_at = ?, deprecation_reason = ?,
                download_count = ?, rating = ?, review_status = ?, security_scan_results = ?, license_compliance = ?, trust_score = ?
             WHERE artifact_id = ? AND version = ?"
        )
        .bind(&metadata.name)
        .bind(&metadata.version)
        .bind(format!("{:?}", metadata.artifact_type))
        .bind(&metadata.description)
        .bind(&metadata.author)
        .bind(&metadata.license)
        .bind(&tags_json)
        .bind(&dependencies_json)
        .bind(&metadata.content_hash)
        .bind(&metadata.signature)
        .bind(&metadata.publisher.publisher_id)
        .bind(now as i64)
        .bind(metadata.published_at.map(|t| t as i64))
        .bind(metadata.deprecated_at.map(|t| t as i64))
        .bind(&metadata.deprecation_reason)
        .bind(metadata.download_count as i64)
        .bind(metadata.rating)
        .bind(&review_status_str)
        .bind(&security_scan_results_json)
        .bind(&license_compliance_json)
        .bind(metadata.trust_score)
        .bind(&metadata.artifact_id)
        .bind(&metadata.version)
        .execute(&self.pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        Ok(())
    }

    async fn delete_artifact_metadata(
        &self,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<(), ArtifactRegistryError> {
        if let Some(ver) = version {
            sqlx::query("DELETE FROM artifacts WHERE artifact_id = ? AND version = ?")
                .bind(artifact_id)
                .bind(ver)
                .execute(&self.pool)
                .await
                .map_err(ArtifactRegistryError::Sqlx)?;
        } else {
            sqlx::query("DELETE FROM artifacts WHERE artifact_id = ?")
                .bind(artifact_id)
                .execute(&self.pool)
                .await
                .map_err(ArtifactRegistryError::Sqlx)?;
        }

        Ok(())
    }

    async fn store_publisher_info(
        &self,
        publisher: &PublisherInfo,
    ) -> Result<(), ArtifactRegistryError> {
        let verification_status_str = format!("{:?}", publisher.verification_status);

        sqlx::query(
            "INSERT OR REPLACE INTO publishers (
                publisher_id, name, email, organization, website, public_key,
                reputation_score, verification_status, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&publisher.publisher_id)
        .bind(&publisher.name)
        .bind(&publisher.email)
        .bind(&publisher.organization)
        .bind(&publisher.website)
        .bind(&publisher.public_key)
        .bind(publisher.reputation_score)
        .bind(&verification_status_str)
        .bind(publisher.created_at as i64)
        .execute(&self.pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        Ok(())
    }

    async fn get_publisher_info(
        &self,
        publisher_id: &str,
    ) -> Result<Option<PublisherInfo>, ArtifactRegistryError> {
        let row = sqlx::query(
            "SELECT publisher_id, name, email, organization, website, public_key,
             reputation_score, verification_status, created_at
             FROM publishers WHERE publisher_id = ?",
        )
        .bind(publisher_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        if let Some(row) = row {
            let verification_status_str: String = row.get("verification_status");
            let verification_status = match verification_status_str.as_str() {
                "Unverified" => VerificationStatus::Unverified,
                "Verified" => VerificationStatus::Verified,
                "Trusted" => VerificationStatus::Trusted,
                _ => {
                    return Err(ArtifactRegistryError::InvalidArtifact(
                        "Invalid verification status".to_string(),
                    ))
                }
            };

            let publisher = PublisherInfo {
                publisher_id: row.get("publisher_id"),
                name: row.get("name"),
                email: row.get("email"),
                organization: row.get("organization"),
                website: row.get("website"),
                public_key: row.get("public_key"),
                reputation_score: row.get::<f64, _>("reputation_score"),
                verification_status,
                created_at: row.get::<i64, _>("created_at") as u64,
            };

            Ok(Some(publisher))
        } else {
            Ok(None)
        }
    }

    async fn update_publisher_info(
        &self,
        publisher: &PublisherInfo,
    ) -> Result<(), ArtifactRegistryError> {
        let verification_status_str = format!("{:?}", publisher.verification_status);

        sqlx::query(
            "UPDATE publishers SET
                name = ?, email = ?, organization = ?, website = ?, public_key = ?,
                reputation_score = ?, verification_status = ?, created_at = ?
             WHERE publisher_id = ?",
        )
        .bind(&publisher.name)
        .bind(&publisher.email)
        .bind(&publisher.organization)
        .bind(&publisher.website)
        .bind(&publisher.public_key)
        .bind(publisher.reputation_score)
        .bind(&verification_status_str)
        .bind(publisher.created_at as i64)
        .bind(&publisher.publisher_id)
        .execute(&self.pool)
        .await
        .map_err(ArtifactRegistryError::Sqlx)?;

        Ok(())
    }

    async fn search_artifacts(
        &self,
        filters: &ArtifactFilters,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ArtifactMetadata>, ArtifactRegistryError> {
        let mut query = "SELECT * FROM artifacts WHERE 1=1".to_string();
        let mut params = Vec::new();

        if let Some(ref name) = filters.name {
            query.push_str(" AND name LIKE ?");
            params.push(format!("%{}%", name));
        }

        if let Some(ref version) = filters.version {
            query.push_str(" AND version = ?");
            params.push(version.clone());
        }

        if let Some(ref artifact_type) = filters.artifact_type {
            query.push_str(" AND artifact_type = ?");
            params.push(format!("{:?}", artifact_type));
        }

        if let Some(ref author) = filters.author {
            query.push_str(" AND author = ?");
            params.push(author.clone());
        }

        if let Some(ref publisher_id) = filters.publisher_id {
            query.push_str(" AND publisher_id = ?");
            params.push(publisher_id.clone());
        }

        if let Some(min_trust_score) = filters.min_trust_score {
            query.push_str(" AND trust_score >= ?");
            params.push(min_trust_score.to_string());
        }

        if let Some(min_rating) = filters.min_rating {
            query.push_str(" AND rating >= ?");
            params.push(min_rating.to_string());
        }

        if let Some(license_approved) = filters.license_approved {
            if license_approved {
                query.push_str(" AND license_compliance LIKE '%\"license_approved\":true%'");
            } else {
                query.push_str(" AND license_compliance LIKE '%\"license_approved\":false%'");
            }
        }

        if let Some(ref review_status) = filters.review_status {
            query.push_str(" AND review_status = ?");
            params.push(format!("{:?}", review_status));
        }

        if let Some(created_after) = filters.created_after {
            query.push_str(" AND created_at > ?");
            params.push(created_after.to_string());
        }

        if let Some(created_before) = filters.created_before {
            query.push_str(" AND created_at < ?");
            params.push(created_before.to_string());
        }

        if let Some(updated_after) = filters.updated_after {
            query.push_str(" AND updated_at > ?");
            params.push(updated_after.to_string());
        }

        if let Some(updated_before) = filters.updated_before {
            query.push_str(" AND updated_at < ?");
            params.push(updated_before.to_string());
        }

        // Add sorting
        query.push_str(" ORDER BY created_at DESC");

        // Add pagination
        if let Some(limit_val) = limit {
            query.push_str(&format!(" LIMIT {}", limit_val));
        }

        if let Some(offset_val) = offset {
            query.push_str(&format!(" OFFSET {}", offset_val));
        }

        let mut db_query = sqlx::query(&query);
        for param in params {
            db_query = db_query.bind(param);
        }

        let rows = db_query
            .fetch_all(&self.pool)
            .await
            .map_err(ArtifactRegistryError::Sqlx)?;

        let mut artifacts = Vec::new();
        for row in rows {
            let tags: Vec<String> = serde_json::from_str(row.get::<&str, _>("tags"))
                .map_err(ArtifactRegistryError::Json)?;
            let dependencies: Vec<ArtifactDependency> =
                serde_json::from_str(row.get::<&str, _>("dependencies"))
                    .map_err(ArtifactRegistryError::Json)?;

            let review_status_str: String = row.get("review_status");
            let review_status = match review_status_str.as_str() {
                "Pending" => ReviewStatus::Pending,
                "Approved" => ReviewStatus::Approved,
                "Rejected" => ReviewStatus::Rejected,
                "Suspended" => ReviewStatus::Suspended,
                _ => {
                    return Err(ArtifactRegistryError::InvalidArtifact(
                        "Invalid review status".to_string(),
                    ))
                }
            };

            let security_scan_results: Option<SecurityScanResults> =
                if let Some(json_str) = row.get::<Option<&str>, _>("security_scan_results") {
                    Some(serde_json::from_str(json_str).map_err(ArtifactRegistryError::Json)?)
                } else {
                    None
                };

            let license_compliance: LicenseCompliance =
                serde_json::from_str(row.get::<&str, _>("license_compliance"))
                    .map_err(ArtifactRegistryError::Json)?;

            // Get publisher info
            let publisher_id: String = row.get("publisher_id");
            let publisher = self
                .get_publisher_info(&publisher_id)
                .await?
                .ok_or_else(|| ArtifactRegistryError::PublisherNotFound(publisher_id.clone()))?;

            let artifact_type_str: String = row.get("artifact_type");
            let artifact_type = match artifact_type_str.as_str() {
                "Skill" => ArtifactType::Skill,
                "Persona" => ArtifactType::Persona,
                "Workflow" => ArtifactType::Workflow,
                "PolicyBundle" => ArtifactType::PolicyBundle,
                "Package" => ArtifactType::Package,
                custom => ArtifactType::Custom(custom.to_string()),
            };

            let artifact = ArtifactMetadata {
                artifact_id: row.get("artifact_id"),
                name: row.get("name"),
                version: row.get("version"),
                artifact_type,
                description: row.get("description"),
                author: row.get("author"),
                license: row.get("license"),
                tags,
                dependencies,
                content_hash: row.get("content_hash"),
                signature: row.get("signature"),
                publisher,
                created_at: row.get::<i64, _>("created_at") as u64,
                updated_at: row.get::<i64, _>("updated_at") as u64,
                published_at: row.get::<Option<i64>, _>("published_at").map(|t| t as u64),
                deprecated_at: row.get::<Option<i64>, _>("deprecated_at").map(|t| t as u64),
                deprecation_reason: row.get("deprecation_reason"),
                download_count: row.get::<i64, _>("download_count") as u64,
                rating: row.get("rating"),
                review_status,
                security_scan_results,
                license_compliance,
                trust_score: row.get::<f64, _>("trust_score"),
            };

            artifacts.push(artifact);
        }

        Ok(artifacts)
    }

    async fn get_artifact_stats(
        &self,
        artifact_id: &str,
    ) -> Result<ArtifactStats, ArtifactRegistryError> {
        // In a real implementation, this would fetch from a stats table
        // For now, return default stats
        Ok(ArtifactStats {
            artifact_id: artifact_id.to_string(),
            total_downloads: 0,
            total_ratings: 0,
            average_rating: 0.0,
            last_downloaded: None,
            last_published: None,
            trust_score: 0.0,
            security_score: 0.0,
            license_compliance_score: 0.0,
        })
    }

    async fn update_artifact_stats(
        &self,
        stats: &ArtifactStats,
    ) -> Result<(), ArtifactRegistryError> {
        // In a real implementation, this would update a stats table
        // For now, we'll just return Ok
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactStats {
    pub artifact_id: String,
    pub total_downloads: u64,
    pub total_ratings: u64,
    pub average_rating: f64,
    pub last_downloaded: Option<u64>,
    pub last_published: Option<u64>,
    pub trust_score: f64,
    pub security_score: f64,
    pub license_compliance_score: f64,
}

pub struct ArtifactRegistry {
    config: ArtifactRegistryConfig,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    context_router: Arc<ContextRouter>,
    memory_fabric: Arc<MemoryFabric>,
    provider_router: Arc<ProviderRouter>,
    skill_registry: Arc<SkillRegistry>,
    workflow_engine: Arc<WorkflowEngine>,
    embodiment_control_plane: Arc<EmbodimentControlPlane>,
    package_manager: Arc<PackageManager>,
    evaluation_engine: Arc<EvaluationEngine>,
    session_manager: Arc<SessionManager>,
    storage: Arc<dyn ArtifactStorage>,

    // In-memory caches
    artifact_cache: Arc<RwLock<HashMap<String, ArtifactMetadata>>>,
    publisher_cache: Arc<RwLock<HashMap<String, PublisherInfo>>>,
}

impl ArtifactRegistry {
    pub async fn new_with_storage(
        config: ArtifactRegistryConfig,
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        context_router: Arc<ContextRouter>,
        memory_fabric: Arc<MemoryFabric>,
        provider_router: Arc<ProviderRouter>,
        skill_registry: Arc<SkillRegistry>,
        workflow_engine: Arc<WorkflowEngine>,
        embodiment_control_plane: Arc<EmbodimentControlPlane>,
        package_manager: Arc<PackageManager>,
        evaluation_engine: Arc<EvaluationEngine>,
        session_manager: Arc<SessionManager>,
        pool: SqlitePool,
    ) -> Result<Self, ArtifactRegistryError> {
        let storage = Arc::new(SqliteArtifactStorage::new(pool).await?);

        // Load existing artifacts and publishers from storage
        let mut artifact_cache = HashMap::new();
        let mut publisher_cache = HashMap::new();

        // In a real implementation, we would load from storage
        // For now, we'll initialize empty caches

        Ok(ArtifactRegistry {
            config,
            history_ledger,
            messaging_system,
            policy_engine,
            context_router,
            memory_fabric,
            provider_router,
            skill_registry,
            workflow_engine,
            embodiment_control_plane,
            package_manager,
            evaluation_engine,
            session_manager,
            storage,
            artifact_cache: Arc::new(RwLock::new(artifact_cache)),
            publisher_cache: Arc::new(RwLock::new(publisher_cache)),
        })
    }

    /// Publish an artifact to the registry
    pub async fn publish_artifact(
        &self,
        request: PublishRequest,
    ) -> Result<PublishResponse, ArtifactRegistryError> {
        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: request.artifact.publisher.publisher_id.clone(),
            resource: format!("artifact_publish:{}", request.artifact.name),
            action: "publish".to_string(),
            context: serde_json::json!({
                "request": &request,
                "artifact": &request.artifact,
                "publisher": &request.artifact.publisher,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for publishing
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(ArtifactRegistryError::AccessDenied(format!(
                "Policy denied artifact publication: {}",
                policy_decision.reason
            )));
        }

        // Validate artifact metadata
        self.validate_artifact_metadata(&request.artifact).await?;

        // Verify signature
        if !self
            .verify_signature(&request.artifact, &request.signature)
            .await?
        {
            return Err(ArtifactRegistryError::SignatureVerificationFailed(
                "Artifact signature verification failed".to_string(),
            ));
        }

        // Perform security scan
        let security_results = self.perform_security_scan(&request.content).await?;
        if !security_results.malware_detected
            && security_results
                .vulnerabilities
                .iter()
                .any(|v| matches!(v.severity, Severity::High | Severity::Critical))
        {
            return Err(ArtifactRegistryError::SecurityScanFailed(
                "Artifact contains high or critical severity vulnerabilities".to_string(),
            ));
        }

        // Check license compliance
        if !self.check_license_compliance(&request.artifact).await? {
            return Err(ArtifactRegistryError::LicenseComplianceFailed(
                "Artifact license does not comply with registry requirements".to_string(),
            ));
        }

        // Calculate content hash
        let mut hasher = Sha256::new();
        hasher.update(&request.content);
        let content_hash = format!("{:x}", hasher.finalize());

        // Update artifact with calculated hash
        let mut artifact = request.artifact.clone();
        artifact.content_hash = content_hash;
        artifact.security_scan_results = Some(security_results);
        artifact.published_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );

        // Store in durable storage
        self.storage.store_artifact_metadata(&artifact).await?;

        // Update cache
        {
            let mut cache = self.artifact_cache.write().await;
            cache.insert(artifact.artifact_id.clone(), artifact.clone());
        }

        // Update publisher in cache
        {
            let mut cache = self.publisher_cache.write().await;
            cache.insert(
                artifact.publisher.publisher_id.clone(),
                artifact.publisher.clone(),
            );
        }

        // Log the publication event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "ArtifactPublished".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: artifact.publisher.publisher_id.clone(), // Publisher identity
            role: "artifact_registry".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id.clone(),
            payload: serde_json::json!({
                "request_id": request.request_id,
                "artifact_id": artifact.artifact_id,
                "name": artifact.name,
                "version": artifact.version,
                "artifact_type": format!("{:?}", artifact.artifact_type),
                "publisher_id": artifact.publisher.publisher_id,
                "content_hash": artifact.content_hash,
                "security_scan_passed": true,
                "license_compliance_passed": true,
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

        Ok(PublishResponse {
            request_id: request.request_id,
            artifact_id: artifact.artifact_id,
            version: artifact.version,
            status: PublishStatus::Success,
            message: "Artifact published successfully".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id,
        })
    }

    async fn validate_artifact_metadata(
        &self,
        metadata: &ArtifactMetadata,
    ) -> Result<(), ArtifactRegistryError> {
        // Validate artifact ID
        if metadata.artifact_id.is_empty() {
            return Err(ArtifactRegistryError::InvalidArtifact(
                "Artifact ID cannot be empty".to_string(),
            ));
        }

        // Validate name
        if metadata.name.is_empty() {
            return Err(ArtifactRegistryError::InvalidArtifact(
                "Artifact name cannot be empty".to_string(),
            ));
        }

        // Validate version (simple semver check)
        let version_parts: Vec<&str> = metadata.version.split('.').collect();
        if version_parts.len() != 3 {
            return Err(ArtifactRegistryError::InvalidArtifact(
                "Version must be in semver format (x.y.z)".to_string(),
            ));
        }

        // Validate author
        if metadata.author.is_empty() {
            return Err(ArtifactRegistryError::InvalidArtifact(
                "Author cannot be empty".to_string(),
            ));
        }

        // Validate license
        if metadata.license.is_empty() {
            return Err(ArtifactRegistryError::InvalidArtifact(
                "License cannot be empty".to_string(),
            ));
        }

        // Validate publisher
        if metadata.publisher.publisher_id.is_empty() {
            return Err(ArtifactRegistryError::InvalidArtifact(
                "Publisher ID cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    async fn verify_signature(
        &self,
        artifact: &ArtifactMetadata,
        signature: &str,
    ) -> Result<bool, ArtifactRegistryError> {
        // In a real implementation, this would verify the digital signature
        // For now, we'll return true to simulate successful verification
        Ok(true)
    }

    async fn perform_security_scan(
        &self,
        content: &[u8],
    ) -> Result<SecurityScanResults, ArtifactRegistryError> {
        // In a real implementation, this would perform a security scan
        // For now, we'll return a clean scan result
        Ok(SecurityScanResults {
            scan_id: Uuid::new_v4().to_string(),
            scanner: "internal_scanner".to_string(),
            scan_date: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            vulnerabilities: vec![],
            malware_detected: false,
            security_score: 1.0,
            recommendations: vec![],
        })
    }

    async fn check_license_compliance(
        &self,
        artifact: &ArtifactMetadata,
    ) -> Result<bool, ArtifactRegistryError> {
        // In a real implementation, this would check license compliance
        // For now, we'll return true to simulate compliance
        Ok(true)
    }

    /// Query artifacts from the registry
    pub async fn query_artifacts(
        &self,
        query: ArtifactQuery,
    ) -> Result<ArtifactQueryResponse, ArtifactRegistryError> {
        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: "system".to_string(), // In real implementation, this would be the querying user/agent
            resource: "artifact_query".to_string(),
            action: "read".to_string(),
            context: serde_json::json!({
                "query": &query,
                "filters": &query.filters,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for querying
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(ArtifactRegistryError::AccessDenied(format!(
                "Policy denied artifact query: {}",
                policy_decision.reason
            )));
        }

        // Search in storage
        let artifacts = self
            .storage
            .search_artifacts(&query.filters, query.limit, query.offset)
            .await?;

        let response = ArtifactQueryResponse {
            query_id: query.query_id,
            artifacts,
            total_count: 0, // In a real implementation, this would be the actual count
            returned_count: 0, // In a real implementation, this would be the actual count
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: query.trace_id,
        };

        // Log the query event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "ArtifactQueryExecuted".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Querying identity
            role: "artifact_registry".to_string(),
            timestamp: response.timestamp,
            trace_id: response.trace_id.clone(),
            payload: serde_json::json!({
                "query_id": response.query_id,
                "filter_count": response.artifacts.len(),
                "total_count": response.total_count,
                "returned_count": response.returned_count,
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

        Ok(response)
    }

    /// Download an artifact from the registry
    pub async fn download_artifact(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadResponse, ArtifactRegistryError> {
        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: request.requesting_agent.clone(),
            resource: format!("artifact_download:{}", request.artifact_id),
            action: "download".to_string(),
            context: serde_json::json!({
                "request": &request,
                "artifact_id": &request.artifact_id,
                "version": &request.version,
                "tenant_id": &request.tenant_id,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for downloading
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(ArtifactRegistryError::AccessDenied(format!(
                "Policy denied artifact download: {}",
                policy_decision.reason
            )));
        }

        // Get artifact metadata
        let artifact = self
            .storage
            .get_artifact_metadata(&request.artifact_id, request.version.as_deref())
            .await?
            .ok_or_else(|| ArtifactRegistryError::ArtifactNotFound(request.artifact_id.clone()))?;

        // In a real implementation, this would fetch the actual artifact content from storage
        // For now, we'll return an empty content vector
        let content = vec![]; // In real implementation, fetch from storage

        // Update download count
        let mut updated_artifact = artifact.clone();
        updated_artifact.download_count += 1;
        self.storage
            .update_artifact_metadata(&updated_artifact)
            .await?;

        // Update cache
        {
            let mut cache = self.artifact_cache.write().await;
            cache.insert(artifact.artifact_id.clone(), updated_artifact);
        }

        let response = DownloadResponse {
            request_id: request.request_id,
            artifact_id: artifact.artifact_id,
            version: artifact.version,
            content,
            content_hash: artifact.content_hash,
            download_url: None, // In real implementation, provide a download URL
            expires_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 3600,
            ), // Expires in 1 hour
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id,
        };

        // Log the download event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "ArtifactDownloaded".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: request.tenant_id,
            actor_id: request.requesting_agent,
            role: "artifact_registry".to_string(),
            timestamp: response.timestamp,
            trace_id: response.trace_id.clone(),
            payload: serde_json::json!({
                "request_id": response.request_id,
                "artifact_id": response.artifact_id,
                "version": response.version,
                "content_hash": response.content_hash,
                "download_url": response.download_url,
                "expires_at": response.expires_at,
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

        Ok(response)
    }

    /// Resolve dependencies for an artifact
    pub async fn resolve_dependencies(
        &self,
        request: DependencyResolutionRequest,
    ) -> Result<DependencyResolutionResponse, ArtifactRegistryError> {
        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: "system".to_string(), // In real implementation, this would be the requesting agent
            resource: format!("dependency_resolution:{}", request.artifact_id),
            action: "resolve".to_string(),
            context: serde_json::json!({
                "request": &request,
                "artifact_id": &request.artifact_id,
                "version": &request.version,
                "target_environment": &request.target_environment,
            }),
            requested_tier: allternit_policy::SafetyTier::T0, // Default to lowest tier for dependency resolution
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(ArtifactRegistryError::AccessDenied(format!(
                "Policy denied dependency resolution: {}",
                policy_decision.reason
            )));
        }

        // In a real implementation, this would resolve dependencies based on the request
        // For now, we'll return a placeholder response
        let response = DependencyResolutionResponse {
            request_id: request.request_id,
            resolved_artifacts: vec![], // In real implementation, populate with resolved artifacts
            conflicts: vec![],          // In real implementation, detect and report conflicts
            security_warnings: vec![],  // In real implementation, check security requirements
            license_warnings: vec![],   // In real implementation, check license requirements
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: request.trace_id,
        };

        // Log the resolution event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "DependencyResolutionCompleted".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Resolver identity
            role: "artifact_registry".to_string(),
            timestamp: response.timestamp,
            trace_id: response.trace_id.clone(),
            payload: serde_json::json!({
                "request_id": response.request_id,
                "artifact_id": request.artifact_id,
                "version": request.version,
                "resolved_count": response.resolved_artifacts.len(),
                "conflict_count": response.conflicts.len(),
                "security_warning_count": response.security_warnings.len(),
                "license_warning_count": response.license_warnings.len(),
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

        Ok(response)
    }

    /// Get the registry configuration
    pub fn get_config(&self) -> &ArtifactRegistryConfig {
        &self.config
    }

    /// Get statistics about the registry
    pub async fn get_statistics(&self) -> RegistryStatistics {
        RegistryStatistics {
            total_artifacts: self.artifact_cache.read().await.len() as u64,
            total_publishers: self.publisher_cache.read().await.len() as u64,
            total_downloads: 0, // In real implementation, track actual downloads
            total_publishes: 0, // In real implementation, track actual publishes
        }
    }

    /// Get artifact metadata by ID and optional version
    pub async fn get_artifact_metadata(
        &self,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<Option<ArtifactMetadata>, ArtifactRegistryError> {
        self.storage
            .get_artifact_metadata(artifact_id, version)
            .await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryStatistics {
    pub total_artifacts: u64,
    pub total_publishers: u64,
    pub total_downloads: u64,
    pub total_publishes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_artifact_registry_basic_functionality() {
        // Create temporary database
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();

        // Create temporary history ledger
        let temp_path = format!("/tmp/test_artifact_registry_{}.jsonl", Uuid::new_v4());
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
        let publisher_identity = allternit_policy::Identity {
            id: "test-publisher-001".to_string(),
            identity_type: allternit_policy::IdentityType::SkillPublisherIdentity,
            name: "Test Publisher".to_string(),
            tenant_id: "test-tenant".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["publisher".to_string()],
            permissions: vec![],
        };
        policy_engine
            .register_identity(publisher_identity)
            .await
            .unwrap();
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
        let publish_rule = allternit_policy::PolicyRule {
            id: "rule_allow_publish".to_string(),
            name: "Allow Publish Operations".to_string(),
            description: "Allow artifact publishing in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec!["publish".to_string()],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(publish_rule).await.unwrap();

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

        // Create tool gateway
        let tool_gateway = Arc::new(allternit_tools_gateway::ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create skill registry
        let skill_registry = Arc::new(
            allternit_skills::SkillRegistry::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                tool_gateway.clone(), // Use clone to avoid move
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create task queue
        let task_queue = Arc::new(
            allternit_messaging::TaskQueue::new(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );

        // Create workflow engine
        let workflow_engine = Arc::new(allternit_workflows::WorkflowEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
            policy_engine.clone(),
            tool_gateway.clone(),
            skill_registry.clone(),
            task_queue,
            pool.clone(),
        ));

        // Create embodiment control plane
        let embodiment_control_plane = Arc::new(
            allternit_embodiment::EmbodimentControlPlane::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                provider_router.clone(),
                skill_registry.clone(),
                workflow_engine.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create package manager
        let package_manager = Arc::new(
            allternit_packaging::PackageManager::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                provider_router.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create evaluation engine
        let evaluation_engine = Arc::new(
            allternit_evals::EvaluationEngine::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                provider_router.clone(),
                skill_registry.clone(),
                workflow_engine.clone(),
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

        // Create registry config
        let config = ArtifactRegistryConfig {
            registry_id: "test-registry-001".to_string(),
            name: "Test Artifact Registry".to_string(),
            description: "A test artifact registry for Allternit".to_string(),
            version: "1.0.0".to_string(),
            storage_config: StorageConfig {
                storage_type: StorageType::LocalFileSystem,
                storage_path: "/tmp/test-artifacts".to_string(),
                encryption_enabled: false,
                compression_enabled: false,
                retention_policy: RetentionPolicy {
                    retention_period_days: 30,
                    auto_delete: false,
                    backup_before_delete: true,
                    compliance_hold: false,
                },
                backup_config: BackupConfig {
                    enabled: true,
                    backup_frequency_hours: 24,
                    backup_location: "/tmp/backups".to_string(),
                    encryption_enabled: true,
                    retention_count: 7,
                },
            },
            security_profile: SecurityProfile {
                sensitivity_tier: 2,
                compliance_requirements: vec![ComplianceRequirement::SOC2],
                audit_level: AuditLevel::Standard,
                encryption_required: true,
                network_isolation: NetworkIsolation::Namespace,
            },
            network_config: NetworkConfiguration {
                allowed_origins: vec!["localhost".to_string(), "127.0.0.1".to_string()],
                cors_enabled: true,
                tls_required: false,
                rate_limiting: Some(RateLimitingConfig {
                    requests_per_minute: 100,
                    burst_size: 200,
                    per_ip_limit: true,
                }),
            },
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

        // Create artifact registry
        let registry = Arc::new(
            ArtifactRegistry::new_with_storage(
                config,
                history_ledger,
                messaging_system,
                policy_engine,
                context_router,
                memory_fabric,
                provider_router,
                skill_registry,
                workflow_engine,
                embodiment_control_plane,
                package_manager,
                evaluation_engine,
                session_manager,
                pool,
            )
            .await
            .unwrap(),
        );

        // Test publisher info
        let publisher = PublisherInfo {
            publisher_id: "test-publisher-001".to_string(),
            name: "Test Publisher".to_string(),
            email: "test@example.com".to_string(),
            organization: Some("Test Organization".to_string()),
            website: Some("https://example.com".to_string()),
            public_key: "test-public-key".to_string(),
            reputation_score: 0.95,
            verification_status: VerificationStatus::Verified,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        // Store publisher info
        registry
            .storage
            .store_publisher_info(&publisher)
            .await
            .unwrap();

        // Test artifact metadata
        let artifact = ArtifactMetadata {
            artifact_id: "test-skill-001".to_string(),
            name: "Test Skill".to_string(),
            version: "1.0.0".to_string(),
            artifact_type: ArtifactType::Skill,
            description: "A test skill for Allternit".to_string(),
            author: "Test Author".to_string(),
            license: "MIT".to_string(),
            tags: vec!["test".to_string(), "allternit".to_string()],
            dependencies: vec![],
            content_hash: "test-content-hash".to_string(),
            signature: Some("test-signature".to_string()),
            publisher,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            published_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            deprecated_at: None,
            deprecation_reason: None,
            download_count: 0,
            rating: Some(4.5),
            review_status: ReviewStatus::Approved,
            security_scan_results: Some(SecurityScanResults {
                scan_id: "scan-001".to_string(),
                scanner: "internal_scanner".to_string(),
                scan_date: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                vulnerabilities: vec![],
                malware_detected: false,
                security_score: 1.0,
                recommendations: vec![],
            }),
            license_compliance: LicenseCompliance {
                license_approved: true,
                license_text: "MIT License".to_string(),
                attribution_required: true,
                distribution_allowed: true,
                modification_allowed: true,
                patent_grant: true,
            },
            trust_score: 0.9,
        };

        // Test artifact publishing
        let publish_request = PublishRequest {
            request_id: "publish-request-001".to_string(),
            artifact,
            content: b"test artifact content".to_vec(),
            signature: "test-signature".to_string(),
            verification_token: "test-token".to_string(),
            trace_id: None,
        };

        let publish_response = registry.publish_artifact(publish_request).await.unwrap();
        assert_eq!(publish_response.status, PublishStatus::Success);

        // Test artifact querying
        let query_request = ArtifactQuery {
            query_id: "query-request-001".to_string(),
            filters: ArtifactFilters {
                name: Some("Test Skill".to_string()),
                version: None,
                artifact_type: Some(ArtifactType::Skill),
                tags: vec!["test".to_string()],
                author: Some("Test Author".to_string()),
                publisher_id: Some("test-publisher-001".to_string()),
                min_trust_score: Some(0.5),
                min_rating: Some(4.0),
                license_approved: Some(true),
                review_status: Some(ReviewStatus::Approved),
                created_after: None,
                created_before: None,
                updated_after: None,
                updated_before: None,
            },
            sort_by: SortField::CreatedAt,
            sort_direction: SortDirection::Desc,
            limit: Some(10),
            offset: Some(0),
            include_deprecated: false,
            trace_id: None,
        };

        let query_response = registry.query_artifacts(query_request).await.unwrap();
        assert!(!query_response.artifacts.is_empty());

        // Test statistics
        let stats = registry.get_statistics().await;
        assert!(stats.total_artifacts >= 0);
        assert!(stats.total_publishers >= 1);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
