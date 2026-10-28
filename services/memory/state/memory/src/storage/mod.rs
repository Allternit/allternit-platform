use crate::{
    ConsolidationState, MemoryEntry, MemoryError, MemoryProvenance, MemoryRetentionPolicy,
    MemoryType,
};

// Define the ExistingRow struct used in the v2 implementation
pub struct ExistingRow {
    pub memory_id: String,
    pub created_at: u64,
    pub status: String,
    pub valid_from: Option<u64>,
    pub valid_to: Option<u64>,
    pub confidence: Option<f64>,
    pub authority: Option<String>,
    pub supersedes_memory_id: Option<String>,
}
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::v2::graph::GraphEdge;

// Storage trait for memory fabric
#[async_trait]
pub trait MemoryStorage: Send + Sync {
    async fn store_memory(&self, memory: &MemoryEntry) -> Result<(), MemoryError>;
    async fn retrieve_memory(&self, memory_id: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    async fn update_memory(&self, memory: &MemoryEntry) -> Result<(), MemoryError>;
    async fn delete_memory(&self, memory_id: &str) -> Result<(), MemoryError>;
    async fn query_memory(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;
    async fn get_memory_by_tier(
        &self,
        tenant_id: &str,
        tier: &MemoryType,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;
    async fn get_expired_memory(&self, before_time: u64) -> Result<Vec<MemoryEntry>, MemoryError>;
    async fn get_over_accessed_memory(
        &self,
        max_accesses: u32,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;

    // V2 methods for truth-preserving storage
    async fn store_memory_v2(
        &self,
        tenant_id: &str,
        memory_id: &str,
        memory_type: &str,
        content: &serde_json::Value,
        metadata: &serde_json::Value,
        tags: &[String],
        now: u64,
    ) -> Result<(), MemoryError>;
}

// Query structure for memory storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub tenant_id: String,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub memory_types: Vec<MemoryType>,
    pub max_sensitivity_tier: Option<u8>,
    pub required_tags: Vec<String>,
    pub time_range: Option<(u64, u64)>, // start_time, end_time
    pub content_search: Option<String>,
    pub limit: Option<usize>,
    pub status_filter: Option<String>, // For v2: active, superseded, archived
}

// SQLite-based storage implementation
pub struct SqliteMemoryStorage {
    pool: SqlitePool,
}

impl SqliteMemoryStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, MemoryError> {
        // Create the memory_entries table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_entries (
                memory_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                session_id TEXT,
                agent_id TEXT,
                memory_type TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_accessed INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                sensitivity_tier INTEGER NOT NULL,
                tags TEXT NOT NULL,
                provenance TEXT NOT NULL,
                retention_policy TEXT NOT NULL,
                consolidation_state TEXT NOT NULL,
                created_at_index INTEGER NOT NULL,
                last_accessed_index INTEGER NOT NULL,
                status TEXT DEFAULT 'active',
                valid_from INTEGER,
                valid_to INTEGER,
                confidence REAL DEFAULT 0.75,
                authority TEXT DEFAULT 'agent',
                supersedes_memory_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        // Backfill missing columns for older schemas.
        let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
            sqlx::query_as("PRAGMA table_info(memory_entries)")
                .fetch_all(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        let has_column = |name: &str| columns.iter().any(|col| col.1 == name);
        if !has_column("status") {
            sqlx::query("ALTER TABLE memory_entries ADD COLUMN status TEXT DEFAULT 'active'")
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }
        if !has_column("valid_from") {
            sqlx::query("ALTER TABLE memory_entries ADD COLUMN valid_from INTEGER")
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }
        if !has_column("valid_to") {
            sqlx::query("ALTER TABLE memory_entries ADD COLUMN valid_to INTEGER")
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }
        if !has_column("confidence") {
            sqlx::query("ALTER TABLE memory_entries ADD COLUMN confidence REAL DEFAULT 0.75")
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }
        if !has_column("authority") {
            sqlx::query("ALTER TABLE memory_entries ADD COLUMN authority TEXT DEFAULT 'agent'")
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }
        if !has_column("supersedes_memory_id") {
            sqlx::query("ALTER TABLE memory_entries ADD COLUMN supersedes_memory_id TEXT")
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }

        // Create indexes for better performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_tenant ON memory_entries(tenant_id)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_type ON memory_entries(memory_type)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_session ON memory_entries(session_id)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_agent ON memory_entries(agent_id)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_sensitivity ON memory_entries(sensitivity_tier)",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_created_at ON memory_entries(created_at_index)",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_last_accessed ON memory_entries(last_accessed_index)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_status ON memory_entries(status)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_valid_from ON memory_entries(valid_from)",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_valid_to ON memory_entries(valid_to)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_supersedes ON memory_entries(supersedes_memory_id)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        // Create graph_edges table for relational memory
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS graph_edges (
                edge_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                confidence REAL NOT NULL DEFAULT 0.75,
                authority TEXT NOT NULL DEFAULT 'agent',
                valid_from INTEGER NOT NULL,
                valid_to INTEGER,
                source_memory_id TEXT,
                source_resource_id TEXT,
                last_accessed INTEGER,
                access_count INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_tenant ON graph_edges(tenant_id)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_subject ON graph_edges(subject)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_predicate ON graph_edges(predicate)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_object ON graph_edges(object)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_status ON graph_edges(status)")
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        // Repair rows written by the earlier v2 insert path: it stored status and
        // authority as Debug strings ('Active', 'Agent') that the lowercase
        // filters never matched, and '{}' placeholders that do not parse as
        // provenance / retention policy.
        sqlx::query(
            "UPDATE memory_entries SET status = lower(status)
             WHERE status IN ('Active', 'Superseded', 'Archived')",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        sqlx::query(
            "UPDATE memory_entries SET authority = lower(authority)
             WHERE authority IN ('User', 'Agent', 'System')",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        let retention_json =
            serde_json::to_string(&default_retention_policy()).map_err(MemoryError::Json)?;
        sqlx::query("UPDATE memory_entries SET retention_policy = ? WHERE retention_policy = '{}'")
            .bind(&retention_json)
            .execute(&pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        let placeholder_rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT memory_id, session_id, agent_id, content FROM memory_entries WHERE provenance = '{}'",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        for (memory_id, session_id, agent_id, content) in placeholder_rows {
            let content: serde_json::Value =
                serde_json::from_str(&content).unwrap_or(serde_json::Value::Null);
            let provenance = MemoryProvenance {
                source_session: session_id,
                source_agent: agent_id.unwrap_or_else(|| "agent".to_string()),
                derivation_chain: vec![],
                integrity_hash: content_hash(&content)?,
                signature: None,
            };
            sqlx::query("UPDATE memory_entries SET provenance = ? WHERE memory_id = ?")
                .bind(serde_json::to_string(&provenance).map_err(MemoryError::Json)?)
                .bind(&memory_id)
                .execute(&pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
        }

        Ok(SqliteMemoryStorage { pool })
    }
}

#[async_trait]
impl MemoryStorage for SqliteMemoryStorage {
    async fn store_memory(&self, memory: &MemoryEntry) -> Result<(), MemoryError> {
        // Use v2 semantics to prevent truth loss, keeping the entry's own
        // session/agent, provenance, retention policy and timeline values.
        let now = memory.valid_from.unwrap_or(memory.created_at);
        let incoming = crate::v2::types::MemoryTimeline::new_active(
            now,
            self.parse_authority_internal(Some(&memory.authority)),
            memory.confidence,
        );
        self.store_versioned_internal(
            &memory.tenant_id,
            &memory.memory_id,
            &format!("{:?}", memory.memory_type),
            &memory.content,
            &memory.metadata,
            &memory.tags,
            now,
            incoming,
            Some(RowFields::from_entry(memory)?),
        )
        .await
    }

    // Add the store_memory_v2 function based on the patch
    async fn store_memory_v2(
        &self,
        tenant_id: &str,
        memory_id: &str,
        memory_type: &str,
        content: &serde_json::Value,
        metadata: &serde_json::Value,
        tags: &[String],
        now: u64,
    ) -> Result<(), MemoryError> {
        use crate::v2::types::{MemoryAuthority, MemoryTimeline};

        let incoming = MemoryTimeline::new_active(now, MemoryAuthority::Agent, 0.75);
        self.store_versioned_internal(
            tenant_id, memory_id, memory_type, content, metadata, tags, now, incoming, None,
        )
        .await
    }

    async fn retrieve_memory(&self, memory_id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let row = sqlx::query("SELECT * FROM memory_entries WHERE memory_id = ?")
            .bind(memory_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        if let Some(row) = row {
            let content: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("content")).map_err(MemoryError::Json)?;
            let tags: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("tags")).map_err(MemoryError::Json)?;
            let provenance: MemoryProvenance =
                serde_json::from_str(row.get::<&str, _>("provenance"))
                    .map_err(MemoryError::Json)?;
            let retention_policy: MemoryRetentionPolicy =
                serde_json::from_str(row.get::<&str, _>("retention_policy"))
                    .map_err(MemoryError::Json)?;

            // Parse memory type from string
            let memory_type_str: String = row.get("memory_type");
            let memory_type = match memory_type_str.as_str() {
                "Working" => MemoryType::Working,
                "Episodic" => MemoryType::Episodic,
                "Semantic" => MemoryType::Semantic,
                "Procedural" => MemoryType::Procedural,
                "Declarative" => MemoryType::Declarative,
                "Meta" => MemoryType::Meta,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown memory type: {}",
                        memory_type_str
                    )))
                }
            };

            // Parse consolidation state from string
            let consolidation_state_str: String = row.get("consolidation_state");
            let consolidation_state = match consolidation_state_str.as_str() {
                "Raw" => ConsolidationState::Raw,
                "Candidate" => ConsolidationState::Candidate,
                "Consolidating" => ConsolidationState::Consolidating,
                "Consolidated" => ConsolidationState::Consolidated,
                "Decayed" => ConsolidationState::Decayed,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown consolidation state: {}",
                        consolidation_state_str
                    )))
                }
            };

            // Parse new v2 fields
            let status: String = row.get("status");
            let valid_from: Option<i64> = row.get("valid_from");
            let valid_to: Option<i64> = row.get("valid_to");
            let confidence: f64 = row.get("confidence");
            let authority: String = row.get("authority");
            let supersedes_memory_id: Option<String> = row.get("supersedes_memory_id");

            let memory = MemoryEntry {
                id: row.get("memory_id"),
                memory_id: row.get("memory_id"),
                tenant_id: row.get("tenant_id"),
                session_id: row.get("session_id"),
                agent_id: row.get("agent_id"),
                memory_type,
                content,
                metadata: serde_json::json!({}),
                embedding: None,
                created_at: row.get::<i64, _>("created_at") as u64,
                last_accessed: row.get::<i64, _>("last_accessed") as u64,
                access_count: row.get::<i32, _>("access_count") as u32,
                sensitivity_tier: row.get::<i32, _>("sensitivity_tier") as u8,
                tags,
                provenance,
                retention_policy,
                consolidation_state,
                status,
                valid_from: valid_from.map(|v| v as u64),
                valid_to: valid_to.map(|v| v as u64),
                confidence,
                authority,
                supersedes_memory_id,
            };

            Ok(Some(memory))
        } else {
            Ok(None)
        }
    }

    async fn update_memory(&self, memory: &MemoryEntry) -> Result<(), MemoryError> {
        let content_json = serde_json::to_string(&memory.content).map_err(MemoryError::Json)?;
        let tags_json = serde_json::to_string(&memory.tags).map_err(MemoryError::Json)?;
        let provenance_json =
            serde_json::to_string(&memory.provenance).map_err(MemoryError::Json)?;
        let retention_policy_json =
            serde_json::to_string(&memory.retention_policy).map_err(MemoryError::Json)?;

        sqlx::query(
            "UPDATE memory_entries SET
                tenant_id = ?, session_id = ?, agent_id = ?, memory_type = ?, content = ?,
                created_at = ?, last_accessed = ?, access_count = ?, sensitivity_tier = ?, tags = ?,
                provenance = ?, retention_policy = ?, consolidation_state = ?, last_accessed_index = ?,
                status = ?, valid_from = ?, valid_to = ?, confidence = ?, authority = ?, supersedes_memory_id = ?
            WHERE memory_id = ?"
        )
        .bind(&memory.tenant_id)
        .bind(&memory.session_id)
        .bind(&memory.agent_id)
        .bind(format!("{:?}", memory.memory_type))
        .bind(&content_json)
        .bind(memory.created_at as i64)
        .bind(memory.last_accessed as i64)
        .bind(memory.access_count as i32)
        .bind(memory.sensitivity_tier as i32)
        .bind(&tags_json)
        .bind(&provenance_json)
        .bind(&retention_policy_json)
        .bind(format!("{:?}", memory.consolidation_state))
        .bind(memory.last_accessed as i64)
        .bind(&memory.status)
        .bind(memory.valid_from.map(|v| v as i64))
        .bind(memory.valid_to.map(|v| v as i64))
        .bind(memory.confidence)
        .bind(&memory.authority)
        .bind(&memory.supersedes_memory_id)
        .bind(&memory.memory_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        Ok(())
    }

    async fn delete_memory(&self, memory_id: &str) -> Result<(), MemoryError> {
        sqlx::query("DELETE FROM memory_entries WHERE memory_id = ?")
            .bind(memory_id)
            .execute(&self.pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        Ok(())
    }

    async fn query_memory(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError> {
        // Build the query dynamically based on the query parameters
        let mut base_query = "SELECT * FROM memory_entries WHERE tenant_id = ?".to_string();
        let mut params: Vec<String> = vec![query.tenant_id.clone()];

        // Add session filter if specified
        if let Some(ref session_id) = query.session_id {
            base_query.push_str(" AND session_id = ?");
            params.push(session_id.clone());
        }

        // Add agent filter if specified
        if let Some(ref agent_id) = query.agent_id {
            base_query.push_str(" AND agent_id = ?");
            params.push(agent_id.clone());
        }

        // Add memory type filters if specified
        if !query.memory_types.is_empty() {
            let type_placeholders: Vec<String> = query
                .memory_types
                .iter()
                .map(|t| format!("{:?}", t))
                .collect();
            let type_placeholders_str = type_placeholders.join("', '");
            base_query.push_str(&format!(
                " AND memory_type IN ('{}')",
                type_placeholders_str
            ));
        }

        // Add sensitivity tier filter if specified
        if let Some(max_tier) = query.max_sensitivity_tier {
            base_query.push_str(" AND sensitivity_tier <= ?");
            params.push(max_tier.to_string());
        }

        // Add status filter if specified (for v2)
        if let Some(ref status) = query.status_filter {
            base_query.push_str(&format!(" AND status = '{}'", status));
        } else {
            base_query.push_str(" AND status = 'active'"); // Default to active memories only
        }

        // Add time range filter if specified
        if let Some((start, end)) = query.time_range {
            base_query.push_str(" AND created_at BETWEEN ? AND ?");
            params.push(start.to_string());
            params.push(end.to_string());
        }

        // Add content search if specified
        if let Some(ref search_term) = query.content_search {
            base_query.push_str(" AND content LIKE ?");
            params.push(format!("%{}%", search_term));
        }

        // Add limit if specified
        if let Some(limit) = query.limit {
            base_query.push_str(&format!(" LIMIT {}", limit));
        }

        // Execute the query with parameters
        let mut query_builder = sqlx::query(&base_query);
        for param in &params {
            query_builder = query_builder.bind(param);
        }

        let rows = query_builder
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        let mut memories = Vec::new();
        for row in rows {
            let content: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("content")).map_err(MemoryError::Json)?;
            let tags: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("tags")).map_err(MemoryError::Json)?;
            let provenance: MemoryProvenance =
                serde_json::from_str(row.get::<&str, _>("provenance"))
                    .map_err(MemoryError::Json)?;
            let retention_policy: MemoryRetentionPolicy =
                serde_json::from_str(row.get::<&str, _>("retention_policy"))
                    .map_err(MemoryError::Json)?;

            // Parse memory type from string
            let memory_type_str: String = row.get("memory_type");
            let memory_type = match memory_type_str.as_str() {
                "Working" => MemoryType::Working,
                "Episodic" => MemoryType::Episodic,
                "Semantic" => MemoryType::Semantic,
                "Procedural" => MemoryType::Procedural,
                "Declarative" => MemoryType::Declarative,
                "Meta" => MemoryType::Meta,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown memory type: {}",
                        memory_type_str
                    )))
                }
            };

            // Parse consolidation state from string
            let consolidation_state_str: String = row.get("consolidation_state");
            let consolidation_state = match consolidation_state_str.as_str() {
                "Raw" => ConsolidationState::Raw,
                "Candidate" => ConsolidationState::Candidate,
                "Consolidating" => ConsolidationState::Consolidating,
                "Consolidated" => ConsolidationState::Consolidated,
                "Decayed" => ConsolidationState::Decayed,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown consolidation state: {}",
                        consolidation_state_str
                    )))
                }
            };

            // Parse new v2 fields
            let status: String = row.get("status");
            let valid_from: Option<i64> = row.get("valid_from");
            let valid_to: Option<i64> = row.get("valid_to");
            let confidence: f64 = row.get("confidence");
            let authority: String = row.get("authority");
            let supersedes_memory_id: Option<String> = row.get("supersedes_memory_id");

            let memory = MemoryEntry {
                id: row.get("memory_id"),
                memory_id: row.get("memory_id"),
                tenant_id: row.get("tenant_id"),
                session_id: row.get("session_id"),
                agent_id: row.get("agent_id"),
                memory_type,
                content,
                metadata: serde_json::json!({}),
                embedding: None,
                created_at: row.get::<i64, _>("created_at") as u64,
                last_accessed: row.get::<i64, _>("last_accessed") as u64,
                access_count: row.get::<i32, _>("access_count") as u32,
                sensitivity_tier: row.get::<i32, _>("sensitivity_tier") as u8,
                tags,
                provenance,
                retention_policy,
                consolidation_state,
                status,
                valid_from: valid_from.map(|v| v as u64),
                valid_to: valid_to.map(|v| v as u64),
                confidence,
                authority,
                supersedes_memory_id,
            };

            memories.push(memory);
        }

        Ok(memories)
    }

    async fn get_memory_by_tier(
        &self,
        tenant_id: &str,
        tier: &MemoryType,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let tier_str = format!("{:?}", tier);
        let rows =
            sqlx::query("SELECT * FROM memory_entries WHERE tenant_id = ? AND memory_type = ? AND status = 'active'")
                .bind(tenant_id)
                .bind(&tier_str)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        let mut memories = Vec::new();
        for row in rows {
            let content: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("content")).map_err(MemoryError::Json)?;
            let tags: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("tags")).map_err(MemoryError::Json)?;
            let provenance: MemoryProvenance =
                serde_json::from_str(row.get::<&str, _>("provenance"))
                    .map_err(MemoryError::Json)?;
            let retention_policy: MemoryRetentionPolicy =
                serde_json::from_str(row.get::<&str, _>("retention_policy"))
                    .map_err(MemoryError::Json)?;

            // Parse memory type from string
            let memory_type_str: String = row.get("memory_type");
            let memory_type = match memory_type_str.as_str() {
                "Working" => MemoryType::Working,
                "Episodic" => MemoryType::Episodic,
                "Semantic" => MemoryType::Semantic,
                "Procedural" => MemoryType::Procedural,
                "Declarative" => MemoryType::Declarative,
                "Meta" => MemoryType::Meta,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown memory type: {}",
                        memory_type_str
                    )))
                }
            };

            // Parse consolidation state from string
            let consolidation_state_str: String = row.get("consolidation_state");
            let consolidation_state = match consolidation_state_str.as_str() {
                "Raw" => ConsolidationState::Raw,
                "Candidate" => ConsolidationState::Candidate,
                "Consolidating" => ConsolidationState::Consolidating,
                "Consolidated" => ConsolidationState::Consolidated,
                "Decayed" => ConsolidationState::Decayed,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown consolidation state: {}",
                        consolidation_state_str
                    )))
                }
            };

            // Parse new v2 fields
            let status: String = row.get("status");
            let valid_from: Option<i64> = row.get("valid_from");
            let valid_to: Option<i64> = row.get("valid_to");
            let confidence: f64 = row.get("confidence");
            let authority: String = row.get("authority");
            let supersedes_memory_id: Option<String> = row.get("supersedes_memory_id");

            let memory = MemoryEntry {
                id: row.get("memory_id"),
                memory_id: row.get("memory_id"),
                tenant_id: row.get("tenant_id"),
                session_id: row.get("session_id"),
                agent_id: row.get("agent_id"),
                memory_type,
                content,
                metadata: serde_json::json!({}),
                embedding: None,
                created_at: row.get::<i64, _>("created_at") as u64,
                last_accessed: row.get::<i64, _>("last_accessed") as u64,
                access_count: row.get::<i32, _>("access_count") as u32,
                sensitivity_tier: row.get::<i32, _>("sensitivity_tier") as u8,
                tags,
                provenance,
                retention_policy,
                consolidation_state,
                status,
                valid_from: valid_from.map(|v| v as u64),
                valid_to: valid_to.map(|v| v as u64),
                confidence,
                authority,
                supersedes_memory_id,
            };

            memories.push(memory);
        }

        Ok(memories)
    }

    async fn get_expired_memory(&self, before_time: u64) -> Result<Vec<MemoryEntry>, MemoryError> {
        let rows = sqlx::query("SELECT * FROM memory_entries WHERE created_at < ?")
            .bind(before_time as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        let mut memories = Vec::new();
        for row in rows {
            let content: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("content")).map_err(MemoryError::Json)?;
            let tags: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("tags")).map_err(MemoryError::Json)?;
            let provenance: MemoryProvenance =
                serde_json::from_str(row.get::<&str, _>("provenance"))
                    .map_err(MemoryError::Json)?;
            let retention_policy: MemoryRetentionPolicy =
                serde_json::from_str(row.get::<&str, _>("retention_policy"))
                    .map_err(MemoryError::Json)?;

            // Parse memory type from string
            let memory_type_str: String = row.get("memory_type");
            let memory_type = match memory_type_str.as_str() {
                "Working" => MemoryType::Working,
                "Episodic" => MemoryType::Episodic,
                "Semantic" => MemoryType::Semantic,
                "Procedural" => MemoryType::Procedural,
                "Declarative" => MemoryType::Declarative,
                "Meta" => MemoryType::Meta,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown memory type: {}",
                        memory_type_str
                    )))
                }
            };

            // Parse consolidation state from string
            let consolidation_state_str: String = row.get("consolidation_state");
            let consolidation_state = match consolidation_state_str.as_str() {
                "Raw" => ConsolidationState::Raw,
                "Candidate" => ConsolidationState::Candidate,
                "Consolidating" => ConsolidationState::Consolidating,
                "Consolidated" => ConsolidationState::Consolidated,
                "Decayed" => ConsolidationState::Decayed,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown consolidation state: {}",
                        consolidation_state_str
                    )))
                }
            };

            // Parse new v2 fields
            let status: String = row.get("status");
            let valid_from: Option<i64> = row.get("valid_from");
            let valid_to: Option<i64> = row.get("valid_to");
            let confidence: f64 = row.get("confidence");
            let authority: String = row.get("authority");
            let supersedes_memory_id: Option<String> = row.get("supersedes_memory_id");

            let memory = MemoryEntry {
                id: row.get("memory_id"),
                memory_id: row.get("memory_id"),
                tenant_id: row.get("tenant_id"),
                session_id: row.get("session_id"),
                agent_id: row.get("agent_id"),
                memory_type,
                content,
                metadata: serde_json::json!({}),
                embedding: None,
                created_at: row.get::<i64, _>("created_at") as u64,
                last_accessed: row.get::<i64, _>("last_accessed") as u64,
                access_count: row.get::<i32, _>("access_count") as u32,
                sensitivity_tier: row.get::<i32, _>("sensitivity_tier") as u8,
                tags,
                provenance,
                retention_policy,
                consolidation_state,
                status,
                valid_from: valid_from.map(|v| v as u64),
                valid_to: valid_to.map(|v| v as u64),
                confidence,
                authority,
                supersedes_memory_id,
            };

            memories.push(memory);
        }

        Ok(memories)
    }

    async fn get_over_accessed_memory(
        &self,
        max_accesses: u32,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let rows = sqlx::query("SELECT * FROM memory_entries WHERE access_count > ?")
            .bind(max_accesses as i32)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        let mut memories = Vec::new();
        for row in rows {
            let content: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("content")).map_err(MemoryError::Json)?;
            let tags: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("tags")).map_err(MemoryError::Json)?;
            let provenance: MemoryProvenance =
                serde_json::from_str(row.get::<&str, _>("provenance"))
                    .map_err(MemoryError::Json)?;
            let retention_policy: MemoryRetentionPolicy =
                serde_json::from_str(row.get::<&str, _>("retention_policy"))
                    .map_err(MemoryError::Json)?;

            // Parse memory type from string
            let memory_type_str: String = row.get("memory_type");
            let memory_type = match memory_type_str.as_str() {
                "Working" => MemoryType::Working,
                "Episodic" => MemoryType::Episodic,
                "Semantic" => MemoryType::Semantic,
                "Procedural" => MemoryType::Procedural,
                "Declarative" => MemoryType::Declarative,
                "Meta" => MemoryType::Meta,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown memory type: {}",
                        memory_type_str
                    )))
                }
            };

            // Parse consolidation state from string
            let consolidation_state_str: String = row.get("consolidation_state");
            let consolidation_state = match consolidation_state_str.as_str() {
                "Raw" => ConsolidationState::Raw,
                "Candidate" => ConsolidationState::Candidate,
                "Consolidating" => ConsolidationState::Consolidating,
                "Consolidated" => ConsolidationState::Consolidated,
                "Decayed" => ConsolidationState::Decayed,
                _ => {
                    return Err(MemoryError::InvalidOperation(format!(
                        "Unknown consolidation state: {}",
                        consolidation_state_str
                    )))
                }
            };

            // Parse new v2 fields
            let status: String = row.get("status");
            let valid_from: Option<i64> = row.get("valid_from");
            let valid_to: Option<i64> = row.get("valid_to");
            let confidence: f64 = row.get("confidence");
            let authority: String = row.get("authority");
            let supersedes_memory_id: Option<String> = row.get("supersedes_memory_id");

            let memory = MemoryEntry {
                id: row.get("memory_id"),
                memory_id: row.get("memory_id"),
                tenant_id: row.get("tenant_id"),
                session_id: row.get("session_id"),
                agent_id: row.get("agent_id"),
                memory_type,
                content,
                metadata: serde_json::json!({}),
                embedding: None,
                created_at: row.get::<i64, _>("created_at") as u64,
                last_accessed: row.get::<i64, _>("last_accessed") as u64,
                access_count: row.get::<i32, _>("access_count") as u32,
                sensitivity_tier: row.get::<i32, _>("sensitivity_tier") as u8,
                tags,
                provenance,
                retention_policy,
                consolidation_state,
                status,
                valid_from: valid_from.map(|v| v as u64),
                valid_to: valid_to.map(|v| v as u64),
                confidence,
                authority,
                supersedes_memory_id,
            };

            memories.push(memory);
        }

        Ok(memories)
    }
}

// Add the helper methods to the SqliteMemoryStorage implementation
impl SqliteMemoryStorage {
    // Private helper methods for v2 functionality
    async fn find_active_by_slot_internal(
        &self,
        tenant: &str,
        slot_key: &str,
    ) -> Result<Option<ExistingRow>, MemoryError> {
        // This is a placeholder implementation - needs to be filled in based on slot_key logic
        // For now, return None to fall back to memory_id lookup
        Ok(None)
    }

    async fn find_active_by_memory_id_internal(
        &self,
        tenant: &str,
        memory_id: &str,
    ) -> Result<Option<ExistingRow>, MemoryError> {
        let row = sqlx::query("SELECT * FROM memory_entries WHERE tenant_id = ? AND memory_id = ? AND status = 'active'")
            .bind(tenant)
            .bind(memory_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        if let Some(row) = row {
            Ok(Some(ExistingRow {
                memory_id: row.get("memory_id"),
                created_at: row.get::<i64, _>("created_at") as u64,
                status: row.get("status"),
                valid_from: row.get::<Option<i64>, _>("valid_from").map(|v| v as u64),
                valid_to: row.get::<Option<i64>, _>("valid_to").map(|v| v as u64),
                confidence: row.get::<Option<f64>, _>("confidence"),
                authority: row.get::<Option<String>, _>("authority"),
                supersedes_memory_id: row.get::<Option<String>, _>("supersedes_memory_id"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn update_timeline_internal(
        &self,
        tenant: &str,
        memory_id: &str,
        tl: &crate::v2::types::MemoryTimeline,
    ) -> Result<(), MemoryError> {
        sqlx::query(
            "UPDATE memory_entries SET
                status = ?,
                valid_to = ?,
                confidence = ?,
                authority = ?,
                supersedes_memory_id = ?
            WHERE tenant_id = ? AND memory_id = ?",
        )
        .bind(status_str(&tl.status))
        .bind(tl.valid_to.map(|v| v as i64))
        .bind(tl.confidence)
        .bind(authority_str(&tl.authority))
        .bind(&tl.supersedes_memory_id)
        .bind(tenant)
        .bind(memory_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        Ok(())
    }

    async fn insert_memory_row_internal(
        &self,
        tenant: &str,
        memory_id: &str,
        memory_type: &str,
        content: &serde_json::Value,
        metadata: &serde_json::Value,
        tags: &[String],
        tl: &crate::v2::types::MemoryTimeline,
        fields: &RowFields,
    ) -> Result<(), MemoryError> {
        let content_json = serde_json::to_string(content).map_err(MemoryError::Json)?;
        let tags_json = serde_json::to_string(tags).map_err(MemoryError::Json)?;

        sqlx::query(
            "INSERT INTO memory_entries (
                memory_id, tenant_id, session_id, agent_id, memory_type, content, created_at, last_accessed,
                access_count, sensitivity_tier, tags, provenance, retention_policy,
                consolidation_state, created_at_index, last_accessed_index,
                status, valid_from, valid_to, confidence, authority, supersedes_memory_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(memory_id)
        .bind(tenant)
        .bind(&fields.session_id)
        .bind(&fields.agent_id)
        .bind(memory_type)
        .bind(&content_json)
        .bind(fields.created_at as i64)
        .bind(fields.last_accessed as i64)
        .bind(fields.access_count as i32)
        .bind(fields.sensitivity_tier as i32)
        .bind(&tags_json)
        .bind(&fields.provenance_json)
        .bind(&fields.retention_policy_json)
        .bind(&fields.consolidation_state)
        .bind(fields.created_at as i64)
        .bind(fields.last_accessed as i64)
        .bind(status_str(&tl.status))
        .bind(tl.valid_from as i64)
        .bind(tl.valid_to.map(|v| v as i64))
        .bind(tl.confidence)
        .bind(authority_str(&tl.authority))
        .bind(&tl.supersedes_memory_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        Ok(())
    }

    /// Shared v2 write path: supersede the active version (if any) instead of
    /// overwriting it, then insert the incoming version. `fields` carries the
    /// non-timeline columns from a full `MemoryEntry`; when absent they are
    /// inherited from the superseded row, or defaulted for a brand-new memory.
    async fn store_versioned_internal(
        &self,
        tenant_id: &str,
        memory_id: &str,
        memory_type: &str,
        content: &serde_json::Value,
        metadata: &serde_json::Value,
        tags: &[String],
        now: u64,
        mut incoming: crate::v2::types::MemoryTimeline,
        fields: Option<RowFields>,
    ) -> Result<(), MemoryError> {
        use crate::v2::conflict::choose_strategy;
        use crate::v2::truth_engine::TruthEngine;
        use crate::v2::types::MemoryTimeline;

        let slot = TruthEngine::slot_key(content, tags, metadata);

        // Find existing active memory by slot, falling back to memory_id
        let existing = match slot.as_ref() {
            Some(slot_key) => self.find_active_by_slot_internal(tenant_id, slot_key).await?,
            None => None,
        };
        let existing = match existing {
            Some(ex) => Some(ex),
            None => {
                self.find_active_by_memory_id_internal(tenant_id, memory_id)
                    .await?
            }
        };

        if let Some(ex) = existing {
            let mut existing_tl = MemoryTimeline {
                status: self.parse_status_internal(&ex.status),
                valid_from: ex.valid_from.unwrap_or(ex.created_at),
                valid_to: ex.valid_to,
                confidence: ex.confidence.unwrap_or(0.75),
                authority: self.parse_authority_internal(ex.authority.as_deref()),
                supersedes_memory_id: ex.supersedes_memory_id.clone(),
            };

            let strategy = choose_strategy(memory_type, tags);
            TruthEngine::apply(now, &mut existing_tl, &mut incoming, strategy);

            let fields = match fields {
                Some(f) => f,
                None => {
                    self.inherit_fields_internal(tenant_id, &ex.memory_id, content, now)
                        .await?
                }
            };

            self.update_timeline_internal(tenant_id, &ex.memory_id, &existing_tl)
                .await?;
            let new_memory_id = format!("{}::v{}", memory_id, now);
            incoming.supersedes_memory_id = Some(ex.memory_id.clone());
            return self
                .insert_memory_row_internal(
                    tenant_id,
                    &new_memory_id,
                    memory_type,
                    content,
                    metadata,
                    tags,
                    &incoming,
                    &fields,
                )
                .await;
        }

        let fields = match fields {
            Some(f) => f,
            None => RowFields::defaults(None, None, 0, content, now)?,
        };
        self.insert_memory_row_internal(
            tenant_id, memory_id, memory_type, content, metadata, tags, &incoming, &fields,
        )
        .await
    }

    /// Non-timeline columns for a new version written without a full entry:
    /// carry session/agent, sensitivity, provenance and retention over from the
    /// version it supersedes.
    async fn inherit_fields_internal(
        &self,
        tenant: &str,
        memory_id: &str,
        content: &serde_json::Value,
        now: u64,
    ) -> Result<RowFields, MemoryError> {
        let row = sqlx::query(
            "SELECT session_id, agent_id, sensitivity_tier, provenance, retention_policy
             FROM memory_entries WHERE tenant_id = ? AND memory_id = ?",
        )
        .bind(tenant)
        .bind(memory_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        let Some(row) = row else {
            return RowFields::defaults(None, None, 0, content, now);
        };
        let session_id: Option<String> = row.get("session_id");
        let agent_id: Option<String> = row.get("agent_id");
        let tier = row.get::<i32, _>("sensitivity_tier") as u8;
        let mut fields = RowFields::defaults(session_id, agent_id, tier, content, now)?;

        // Keep the superseded version's provenance (extended with its id) and
        // retention policy when they parse; otherwise keep the defaults.
        if let Ok(mut prov) =
            serde_json::from_str::<MemoryProvenance>(row.get::<&str, _>("provenance"))
        {
            prov.derivation_chain.push(memory_id.to_string());
            prov.integrity_hash = content_hash(content)?;
            prov.signature = None;
            fields.provenance_json = serde_json::to_string(&prov).map_err(MemoryError::Json)?;
        }
        let retention: &str = row.get("retention_policy");
        if serde_json::from_str::<MemoryRetentionPolicy>(retention).is_ok() {
            fields.retention_policy_json = retention.to_string();
        }
        Ok(fields)
    }

    fn parse_status_internal(&self, s: &str) -> crate::v2::types::MemoryStatus {
        match s {
            "active" => crate::v2::types::MemoryStatus::Active,
            "superseded" => crate::v2::types::MemoryStatus::Superseded,
            "archived" => crate::v2::types::MemoryStatus::Archived,
            _ => crate::v2::types::MemoryStatus::Active,
        }
    }

    fn parse_authority_internal(&self, s: Option<&str>) -> crate::v2::types::MemoryAuthority {
        match s.unwrap_or("agent") {
            "user" => crate::v2::types::MemoryAuthority::User,
            "system" => crate::v2::types::MemoryAuthority::System,
            _ => crate::v2::types::MemoryAuthority::Agent,
        }
    }

    pub async fn upsert_graph_edge(
        &self,
        edge: crate::v2::graph::GraphEdge,
    ) -> Result<(), MemoryError> {
        sqlx::query(
            "INSERT OR REPLACE INTO graph_edges (
                edge_id, tenant_id, subject, predicate, object, status, confidence, authority,
                valid_from, valid_to, source_memory_id, source_resource_id, last_accessed, access_count
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&edge.edge_id)
        .bind(&edge.tenant_id)
        .bind(&edge.subject)
        .bind(&edge.predicate)
        .bind(&edge.object)
        .bind(&edge.status)
        .bind(edge.confidence)
        .bind(&edge.authority)
        .bind(edge.valid_from as i64)
        .bind(edge.valid_to.map(|v| v as i64))
        .bind(&edge.source_memory_id)
        .bind(&edge.source_resource_id)
        .bind(edge.last_accessed.map(|v| v as i64))
        .bind(edge.access_count as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        Ok(())
    }

    pub async fn traverse_graph(
        &self,
        query: crate::v2::graph::GraphQuery,
    ) -> Result<Vec<crate::v2::graph::GraphEdge>, MemoryError> {
        // Simple traversal - just find edges matching the seed as subject or object
        let rows = sqlx::query(
            "SELECT * FROM graph_edges
             WHERE tenant_id = ?
             AND (subject = ? OR object = ?)
             AND status = 'active'",
        )
        .bind(&query.tenant_id)
        .bind(&query.seed)
        .bind(&query.seed)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;

        let mut edges = Vec::new();
        for row in rows {
            let edge = crate::v2::graph::GraphEdge {
                edge_id: row.get("edge_id"),
                tenant_id: row.get("tenant_id"),
                subject: row.get("subject"),
                predicate: row.get("predicate"),
                object: row.get("object"),
                status: row.get("status"),
                confidence: row.get::<f64, _>("confidence"),
                authority: row.get("authority"),
                valid_from: row.get::<i64, _>("valid_from") as u64,
                valid_to: row.get::<Option<i64>, _>("valid_to").map(|v| v as u64),
                source_memory_id: row.get("source_memory_id"),
                source_resource_id: row.get("source_resource_id"),
                last_accessed: row.get::<Option<i64>, _>("last_accessed").map(|v| v as u64),
                access_count: row.get::<i64, _>("access_count") as u64,
            };
            edges.push(edge);
        }

        Ok(edges)
    }
}

impl crate::v2::graph::GraphStore for SqliteMemoryStorage {
    fn upsert_edge(&self, edge: crate::v2::graph::GraphEdge) -> anyhow::Result<()> {
        // Since we can't do async in trait implementation, we'll use a blocking approach
        // In a real implementation, we'd want to handle this differently
        use tokio::runtime::Handle;

        match Handle::try_current() {
            Ok(handle) => {
                let pool = self.pool.clone();
                handle.block_on(async {
                    sqlx::query(
                        "INSERT OR REPLACE INTO graph_edges (
                            edge_id, tenant_id, subject, predicate, object, status, confidence, authority,
                            valid_from, valid_to, source_memory_id, source_resource_id, last_accessed, access_count
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    )
                    .bind(&edge.edge_id)
                    .bind(&edge.tenant_id)
                    .bind(&edge.subject)
                    .bind(&edge.predicate)
                    .bind(&edge.object)
                    .bind(&edge.status)
                    .bind(edge.confidence)
                    .bind(&edge.authority)
                    .bind(edge.valid_from as i64)
                    .bind(edge.valid_to.map(|v| v as i64))
                    .bind(&edge.source_memory_id)
                    .bind(&edge.source_resource_id)
                    .bind(edge.last_accessed.map(|v| v as i64))
                    .bind(edge.access_count as i64)
                    .execute(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))
                })?;
            }
            Err(_) => {
                // If we're not in a tokio runtime, we can't do async operations
                return Err(anyhow::anyhow!("Not in tokio runtime"));
            }
        }

        Ok(())
    }

    fn traverse(
        &self,
        query: crate::v2::graph::GraphQuery,
    ) -> anyhow::Result<Vec<crate::v2::graph::GraphEdge>> {
        use tokio::runtime::Handle;

        match Handle::try_current() {
            Ok(handle) => {
                let pool = self.pool.clone();
                let result = handle.block_on(async {
                    let rows = sqlx::query(
                        "SELECT * FROM graph_edges
                         WHERE tenant_id = ?
                         AND (subject = ? OR object = ?)
                         AND status = 'active'",
                    )
                    .bind(&query.tenant_id)
                    .bind(&query.seed)
                    .bind(&query.seed)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

                    let mut edges = Vec::new();
                    for row in rows {
                        let edge = crate::v2::graph::GraphEdge {
                            edge_id: row.get("edge_id"),
                            tenant_id: row.get("tenant_id"),
                            subject: row.get("subject"),
                            predicate: row.get("predicate"),
                            object: row.get("object"),
                            status: row.get("status"),
                            confidence: row.get::<f64, _>("confidence"),
                            authority: row.get("authority"),
                            valid_from: row.get::<i64, _>("valid_from") as u64,
                            valid_to: row.get::<Option<i64>, _>("valid_to").map(|v| v as u64),
                            source_memory_id: row.get("source_memory_id"),
                            source_resource_id: row.get("source_resource_id"),
                            last_accessed: row
                                .get::<Option<i64>, _>("last_accessed")
                                .map(|v| v as u64),
                            access_count: row.get::<i64, _>("access_count") as u64,
                        };
                        edges.push(edge);
                    }

                    Ok::<Vec<GraphEdge>, anyhow::Error>(edges)
                })?;

                Ok(result)
            }
            Err(_) => Err(anyhow::anyhow!("Not in tokio runtime")),
        }
    }
}

/// Columns of a `memory_entries` row that are not part of the v2 timeline.
struct RowFields {
    session_id: Option<String>,
    agent_id: Option<String>,
    created_at: u64,
    last_accessed: u64,
    access_count: u32,
    sensitivity_tier: u8,
    provenance_json: String,
    retention_policy_json: String,
    consolidation_state: String,
}

impl RowFields {
    fn from_entry(memory: &MemoryEntry) -> Result<Self, MemoryError> {
        Ok(Self {
            session_id: memory.session_id.clone(),
            agent_id: memory.agent_id.clone(),
            created_at: memory.created_at,
            last_accessed: memory.last_accessed,
            access_count: memory.access_count,
            sensitivity_tier: memory.sensitivity_tier,
            provenance_json: serde_json::to_string(&memory.provenance)
                .map_err(MemoryError::Json)?,
            retention_policy_json: serde_json::to_string(&memory.retention_policy)
                .map_err(MemoryError::Json)?,
            consolidation_state: format!("{:?}", memory.consolidation_state),
        })
    }

    /// Defaults for a memory written through `store_memory_v2` with no entry:
    /// provenance names the agent and hashes the content; retention keeps the
    /// memory without decay and archives rather than deletes it.
    fn defaults(
        session_id: Option<String>,
        agent_id: Option<String>,
        sensitivity_tier: u8,
        content: &serde_json::Value,
        now: u64,
    ) -> Result<Self, MemoryError> {
        let provenance = MemoryProvenance {
            source_session: session_id.clone(),
            source_agent: agent_id.clone().unwrap_or_else(|| "agent".to_string()),
            derivation_chain: vec![],
            integrity_hash: content_hash(content)?,
            signature: None,
        };
        Ok(Self {
            session_id,
            agent_id,
            created_at: now,
            last_accessed: now,
            access_count: 0,
            sensitivity_tier,
            provenance_json: serde_json::to_string(&provenance).map_err(MemoryError::Json)?,
            retention_policy_json: serde_json::to_string(&default_retention_policy())
                .map_err(MemoryError::Json)?,
            consolidation_state: format!("{:?}", ConsolidationState::Raw),
        })
    }
}

fn default_retention_policy() -> MemoryRetentionPolicy {
    MemoryRetentionPolicy {
        time_to_live: None,
        max_accesses: None,
        decay_function: crate::MemoryDecayFunction::Linear { rate: 0.0 },
        consolidation_trigger: crate::ConsolidationTrigger::Manual,
        deletion_policy: crate::DeletionPolicy::Archival,
    }
}

fn content_hash(content: &serde_json::Value) -> Result<String, MemoryError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(content).map_err(MemoryError::Json)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Status values as the rest of the storage layer reads them ('active', ...).
fn status_str(status: &crate::v2::types::MemoryStatus) -> &'static str {
    use crate::v2::types::MemoryStatus;
    match status {
        MemoryStatus::Active => "active",
        MemoryStatus::Superseded => "superseded",
        MemoryStatus::Archived => "archived",
    }
}

/// Authority values as `parse_authority_internal` reads them ('agent', ...).
fn authority_str(authority: &crate::v2::types::MemoryAuthority) -> &'static str {
    use crate::v2::types::MemoryAuthority;
    match authority {
        MemoryAuthority::User => "user",
        MemoryAuthority::Agent => "agent",
        MemoryAuthority::System => "system",
    }
}
