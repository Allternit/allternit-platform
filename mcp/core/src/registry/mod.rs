//! MCP Server Registry (N13)
//!
//! Persistent storage for MCP server configurations using SQLite.
//! Features:
//! - Server configuration CRUD
//! - OAuth token storage (encrypted)
//! - Health status tracking
//! - Connection state persistence

use crate::error::{McpError, Result};
use crate::transport::TransportType;
use crate::{SseConfig, StdioConfig};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::time::Duration;
use tracing::{debug, info, warn};

pub mod db;

use db::*;

/// MCP server record in registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRecord {
    /// Unique server identifier
    pub server_id: String,
    /// Server display name
    pub name: String,
    /// Server description
    pub description: String,
    /// Transport type
    pub transport_type: TransportType,
    /// Stdio configuration (for stdio transport)
    pub stdio_config: Option<StdioConfig>,
    /// SSE configuration (for SSE transport)
    pub sse_config: Option<SseConfig>,
    /// Whether server is enabled
    pub enabled: bool,
    /// Auto-connect on startup
    pub auto_connect: bool,
    /// Tool name prefix
    pub tool_prefix: Option<String>,
    /// Safety tier (T0-T4)
    pub safety_tier: u8,
    /// Created timestamp
    pub created_at: i64,
    /// Last updated timestamp
    pub updated_at: i64,
}

/// MCP server status record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    /// Server ID
    pub server_id: String,
    /// Connection state
    pub connection_state: ConnectionState,
    /// Last connected timestamp
    pub last_connected_at: Option<i64>,
    /// Last disconnected timestamp
    pub last_disconnected_at: Option<i64>,
    /// Last error message
    pub last_error: Option<String>,
    /// Number of successful connections
    pub connection_count: u64,
    /// Number of failed connections
    pub failure_count: u64,
    /// Health check failures
    pub health_check_failures: u32,
    /// Updated timestamp
    pub updated_at: i64,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "disconnected"),
            ConnectionState::Connecting => write!(f, "connecting"),
            ConnectionState::Connected => write!(f, "connected"),
            ConnectionState::Failed => write!(f, "failed"),
        }
    }
}

/// OAuth token storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenRecord {
    /// Server ID
    pub server_id: String,
    /// Access token (encrypted)
    pub access_token: String,
    /// Refresh token (encrypted)
    pub refresh_token: Option<String>,
    /// Token type (e.g., "Bearer")
    pub token_type: String,
    /// Expiration timestamp
    pub expires_at: Option<i64>,
    /// Scope
    pub scope: Option<String>,
    /// Updated timestamp
    pub updated_at: i64,
}

/// MCP server registry
#[derive(Debug)]
pub struct McpRegistry {
    pool: Pool<Sqlite>,
}

impl McpRegistry {
    /// Create a new registry with an existing pool
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self> {
        let registry = Self { pool };
        registry.init().await?;
        Ok(registry)
    }

    /// Create a new registry with a database URL
    pub async fn from_url(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to connect to database: {e}")))?;

        Self::new(pool).await
    }

    /// Initialize the database schema
    async fn init(&self) -> Result<()> {
        debug!("Initializing MCP registry schema");

        sqlx::query(SCHEMA_SERVERS)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to create servers table: {e}")))?;

        sqlx::query(SCHEMA_STATUS)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to create status table: {e}")))?;

        sqlx::query(SCHEMA_OAUTH)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to create oauth table: {e}")))?;

        info!("MCP registry schema initialized");
        Ok(())
    }

    // =========================================================================
    // Server CRUD Operations
    // =========================================================================

    /// Register or update an MCP server
    pub async fn save_server(&self, server: &McpServerRecord) -> Result<()> {
        debug!("Saving MCP server: {}", server.server_id);

        let stdio_json = server
            .stdio_config
            .as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default());
        let sse_json = server
            .sse_config
            .as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default());
        let transport_str = match server.transport_type {
            TransportType::Stdio => "stdio",
            TransportType::Sse => "sse",
            TransportType::StreamableHttp => "streamable_http",
        };

        sqlx::query(UPSERT_SERVER)
            .bind(&server.server_id)
            .bind(&server.name)
            .bind(&server.description)
            .bind(transport_str)
            .bind(stdio_json.as_deref())
            .bind(sse_json.as_deref())
            .bind(server.enabled)
            .bind(server.auto_connect)
            .bind(server.tool_prefix.as_deref())
            .bind(server.safety_tier as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to save server: {e}")))?;

        // Initialize status record if not exists
        sqlx::query(INIT_STATUS)
            .bind(&server.server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to init status: {e}")))?;

        info!("Saved MCP server: {}", server.server_id);
        Ok(())
    }

    /// Get a server by ID
    pub async fn get_server(&self, server_id: &str) -> Result<Option<McpServerRecord>> {
        let row = sqlx::query_as::<_, ServerRow>(SELECT_SERVER)
            .bind(server_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to get server: {e}")))?;

        match row {
            Some(row) => Ok(Some(self.row_to_server(row)?)),
            None => Ok(None),
        }
    }

    /// List all servers
    pub async fn list_servers(&self) -> Result<Vec<McpServerRecord>> {
        let rows = sqlx::query_as::<_, ServerRow>(SELECT_ALL_SERVERS)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to list servers: {e}")))?;

        rows.into_iter()
            .map(|row| self.row_to_server(row))
            .collect::<Result<Vec<_>>>()
    }

    /// List enabled servers
    pub async fn list_enabled_servers(&self) -> Result<Vec<McpServerRecord>> {
        let rows = sqlx::query_as::<_, ServerRow>(SELECT_ENABLED_SERVERS)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to list enabled servers: {e}")))?;

        rows.into_iter()
            .map(|row| self.row_to_server(row))
            .collect::<Result<Vec<_>>>()
    }

    /// List servers with auto-connect enabled
    pub async fn list_auto_connect_servers(&self) -> Result<Vec<McpServerRecord>> {
        let rows = sqlx::query_as::<_, ServerRow>(SELECT_AUTO_CONNECT_SERVERS)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to list auto-connect servers: {e}")))?;

        rows.into_iter()
            .map(|row| self.row_to_server(row))
            .collect::<Result<Vec<_>>>()
    }

    /// Delete a server
    pub async fn delete_server(&self, server_id: &str) -> Result<()> {
        debug!("Deleting MCP server: {}", server_id);

        // Delete in correct order to avoid FK constraints
        sqlx::query(DELETE_OAUTH)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to delete oauth: {e}")))?;

        sqlx::query(DELETE_STATUS)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to delete status: {e}")))?;

        let result = sqlx::query(DELETE_SERVER)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to delete server: {e}")))?;

        if result.rows_affected() == 0 {
            warn!("Server not found for deletion: {}", server_id);
        } else {
            info!("Deleted MCP server: {}", server_id);
        }

        Ok(())
    }

    // =========================================================================
    // Status Operations
    // =========================================================================

    /// Update server status
    pub async fn update_status(&self, status: &McpServerStatus) -> Result<()> {
        let state_str = status.connection_state.to_string();

        sqlx::query(UPSERT_STATUS)
            .bind(&status.server_id)
            .bind(state_str)
            .bind(status.last_connected_at)
            .bind(status.last_disconnected_at)
            .bind(status.last_error.as_deref())
            .bind(status.connection_count as i64)
            .bind(status.failure_count as i64)
            .bind(status.health_check_failures as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to update status: {e}")))?;

        Ok(())
    }

    /// Get server status
    pub async fn get_status(&self, server_id: &str) -> Result<Option<McpServerStatus>> {
        let row = sqlx::query_as::<_, StatusRow>(SELECT_STATUS)
            .bind(server_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to get status: {e}")))?;

        match row {
            Some(row) => Ok(Some(self.row_to_status(row)?)),
            None => Ok(None),
        }
    }

    /// Record connection success
    pub async fn record_connection_success(&self, server_id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        sqlx::query(RECORD_CONNECTION_SUCCESS)
            .bind(now)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to record success: {e}")))?;

        Ok(())
    }

    /// Record connection failure
    pub async fn record_connection_failure(&self, server_id: &str, error: &str) -> Result<()> {
        sqlx::query(RECORD_CONNECTION_FAILURE)
            .bind(error)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to record failure: {e}")))?;

        Ok(())
    }

    /// Record health check failure
    pub async fn record_health_failure(&self, server_id: &str) -> Result<()> {
        sqlx::query(RECORD_HEALTH_FAILURE)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to record health failure: {e}")))?;

        Ok(())
    }

    /// Reset health check failures
    pub async fn reset_health_failures(&self, server_id: &str) -> Result<()> {
        sqlx::query(RESET_HEALTH_FAILURES)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to reset health failures: {e}")))?;

        Ok(())
    }

    /// Record that a health check was performed (updates timestamp)
    pub async fn record_health_check(&self, server_id: &str) -> Result<()> {
        sqlx::query(
            "
            UPDATE mcp_server_status 
            SET updated_at = strftime('%s', 'now') 
            WHERE server_id = ?
        ",
        )
        .bind(server_id)
        .execute(&self.pool)
        .await
        .map_err(|e| McpError::Storage(format!("Failed to record health check: {e}")))?;

        Ok(())
    }

    // =========================================================================
    // OAuth Token Operations
    // =========================================================================

    /// Save OAuth token (with simple encryption)
    pub async fn save_oauth_token(&self, token: &OAuthTokenRecord) -> Result<()> {
        debug!("Saving OAuth token for server: {}", token.server_id);

        // Simple XOR encryption with a key from environment
        // In production, use proper encryption (e.g., AWS KMS, HashiCorp Vault)
        let encrypted_access = self.encrypt_token(&token.access_token)?;
        let encrypted_refresh = token
            .refresh_token
            .as_ref()
            .map(|t| self.encrypt_token(t))
            .transpose()?;

        sqlx::query(UPSERT_OAUTH)
            .bind(&token.server_id)
            .bind(encrypted_access)
            .bind(encrypted_refresh)
            .bind(&token.token_type)
            .bind(token.expires_at)
            .bind(token.scope.as_deref())
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to save OAuth token: {e}")))?;

        debug!("Saved OAuth token for server: {}", token.server_id);
        Ok(())
    }

    /// Get OAuth token
    pub async fn get_oauth_token(&self, server_id: &str) -> Result<Option<OAuthTokenRecord>> {
        let row = sqlx::query_as::<_, OAuthRow>(SELECT_OAUTH)
            .bind(server_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to get OAuth token: {e}")))?;

        match row {
            Some(row) => Ok(Some(self.row_to_oauth(row)?)),
            None => Ok(None),
        }
    }

    /// Delete OAuth token
    pub async fn delete_oauth_token(&self, server_id: &str) -> Result<()> {
        sqlx::query(DELETE_OAUTH)
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(|e| McpError::Storage(format!("Failed to delete OAuth token: {e}")))?;

        Ok(())
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    fn row_to_server(&self, row: ServerRow) -> Result<McpServerRecord> {
        let transport_type = match row.transport_type.as_str() {
            "stdio" => TransportType::Stdio,
            "sse" => TransportType::Sse,
            "streamable_http" => TransportType::StreamableHttp,
            _ => TransportType::Stdio,
        };

        let stdio_config = row
            .stdio_config
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let sse_config = row
            .sse_config
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(McpServerRecord {
            server_id: row.server_id,
            name: row.name,
            description: row.description,
            transport_type,
            stdio_config,
            sse_config,
            enabled: row.enabled,
            auto_connect: row.auto_connect,
            tool_prefix: row.tool_prefix,
            safety_tier: row.safety_tier as u8,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    fn row_to_status(&self, row: StatusRow) -> Result<McpServerStatus> {
        let connection_state = match row.connection_state.as_str() {
            "connected" => ConnectionState::Connected,
            "connecting" => ConnectionState::Connecting,
            "failed" => ConnectionState::Failed,
            _ => ConnectionState::Disconnected,
        };

        Ok(McpServerStatus {
            server_id: row.server_id,
            connection_state,
            last_connected_at: row.last_connected_at,
            last_disconnected_at: row.last_disconnected_at,
            last_error: row.last_error,
            connection_count: row.connection_count as u64,
            failure_count: row.failure_count as u64,
            health_check_failures: row.health_check_failures as u32,
            updated_at: row.updated_at,
        })
    }

    fn row_to_oauth(&self, row: OAuthRow) -> Result<OAuthTokenRecord> {
        let decrypted_access = self.decrypt_token(&row.access_token)?;
        let decrypted_refresh = row
            .refresh_token
            .as_deref()
            .map(|t| self.decrypt_token(t))
            .transpose()?;

        Ok(OAuthTokenRecord {
            server_id: row.server_id,
            access_token: decrypted_access,
            refresh_token: decrypted_refresh,
            token_type: row.token_type,
            expires_at: row.expires_at,
            scope: row.scope,
            updated_at: row.updated_at,
        })
    }

    /// Simple token encryption (XOR with key)
    /// In production, use proper encryption
    fn encrypt_token(&self, token: &str) -> Result<String> {
        let key = self.get_encryption_key()?;
        let encrypted: Vec<u8> = token
            .bytes()
            .zip(key.iter().cycle())
            .map(|(b, k)| b ^ k)
            .collect();
        Ok(BASE64.encode(encrypted))
    }

    fn decrypt_token(&self, encrypted: &str) -> Result<String> {
        let key = self.get_encryption_key()?;
        let encrypted = BASE64
            .decode(encrypted)
            .map_err(|e| McpError::Storage(format!("Failed to decode token: {e}")))?;
        let decrypted: Vec<u8> = encrypted
            .iter()
            .zip(key.iter().cycle())
            .map(|(b, k)| b ^ k)
            .collect();
        String::from_utf8(decrypted)
            .map_err(|e| McpError::Storage(format!("Failed to decrypt token: {e}")))
    }

    fn get_encryption_key(&self) -> Result<Vec<u8>> {
        // Get key from environment or use default (insecure!)
        let key_str = std::env::var("MCP_ENCRYPTION_KEY")
            .unwrap_or_else(|_| "default_insecure_key_change_in_production".to_string());
        Ok(key_str.into_bytes())
    }

    /// Close the database pool
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    async fn create_test_registry() -> McpRegistry {
        let pool = SqlitePoolOptions::new().connect(":memory:").await.unwrap();
        McpRegistry::new(pool).await.unwrap()
    }

    #[tokio::test]
    async fn test_save_and_get_server() {
        let registry = create_test_registry().await;

        let server = McpServerRecord {
            server_id: "test-server".to_string(),
            name: "Test Server".to_string(),
            description: "A test MCP server".to_string(),
            transport_type: TransportType::Stdio,
            stdio_config: Some(StdioConfig {
                command: "test".to_string(),
                args: vec!["--test".to_string()],
                env: HashMap::new(),
                cwd: None,
                timeout_secs: 30,
            }),
            sse_config: None,
            enabled: true,
            auto_connect: false,
            tool_prefix: Some("test".to_string()),
            safety_tier: 1,
            created_at: 0,
            updated_at: 0,
        };

        registry.save_server(&server).await.unwrap();

        let retrieved = registry.get_server("test-server").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.server_id, "test-server");
        assert_eq!(retrieved.name, "Test Server");
        assert!(retrieved.enabled);
    }

    #[tokio::test]
    async fn test_list_servers() {
        let registry = create_test_registry().await;

        for i in 0..3 {
            let server = McpServerRecord {
                server_id: format!("server-{}", i),
                name: format!("Server {}", i),
                description: "Test".to_string(),
                transport_type: TransportType::Stdio,
                stdio_config: None,
                sse_config: None,
                enabled: i % 2 == 0,
                auto_connect: i == 0,
                tool_prefix: None,
                safety_tier: 1,
                created_at: 0,
                updated_at: 0,
            };
            registry.save_server(&server).await.unwrap();
        }

        let all = registry.list_servers().await.unwrap();
        assert_eq!(all.len(), 3);

        let enabled = registry.list_enabled_servers().await.unwrap();
        assert_eq!(enabled.len(), 2);

        let auto_connect = registry.list_auto_connect_servers().await.unwrap();
        assert_eq!(auto_connect.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_server() {
        let registry = create_test_registry().await;

        let server = McpServerRecord {
            server_id: "delete-me".to_string(),
            name: "Delete Me".to_string(),
            description: "To be deleted".to_string(),
            transport_type: TransportType::Stdio,
            stdio_config: None,
            sse_config: None,
            enabled: true,
            auto_connect: false,
            tool_prefix: None,
            safety_tier: 1,
            created_at: 0,
            updated_at: 0,
        };

        registry.save_server(&server).await.unwrap();
        assert!(registry.get_server("delete-me").await.unwrap().is_some());

        registry.delete_server("delete-me").await.unwrap();
        assert!(registry.get_server("delete-me").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_status_operations() {
        let registry = create_test_registry().await;

        // Create server first
        let server = McpServerRecord {
            server_id: "status-test".to_string(),
            name: "Status Test".to_string(),
            description: "Test".to_string(),
            transport_type: TransportType::Stdio,
            stdio_config: None,
            sse_config: None,
            enabled: true,
            auto_connect: false,
            tool_prefix: None,
            safety_tier: 1,
            created_at: 0,
            updated_at: 0,
        };
        registry.save_server(&server).await.unwrap();

        // Record connection success
        registry
            .record_connection_success("status-test")
            .await
            .unwrap();

        let status = registry.get_status("status-test").await.unwrap();
        assert!(status.is_some());
        let status = status.unwrap();
        assert_eq!(status.connection_state, ConnectionState::Connected);
        assert_eq!(status.connection_count, 1);

        // Record failure
        registry
            .record_connection_failure("status-test", "Test error")
            .await
            .unwrap();
        let status = registry.get_status("status-test").await.unwrap().unwrap();
        assert_eq!(status.connection_state, ConnectionState::Failed);
        assert_eq!(status.failure_count, 1);
        assert_eq!(status.last_error, Some("Test error".to_string()));
    }

    #[tokio::test]
    async fn test_oauth_token_encryption() {
        let registry = create_test_registry().await;

        // Create server first (required for FK constraint)
        let server = McpServerRecord {
            server_id: "oauth-test".to_string(),
            name: "OAuth Test".to_string(),
            description: "Test".to_string(),
            transport_type: TransportType::Stdio,
            stdio_config: None,
            sse_config: None,
            enabled: true,
            auto_connect: false,
            tool_prefix: None,
            safety_tier: 1,
            created_at: 0,
            updated_at: 0,
        };
        registry.save_server(&server).await.unwrap();

        let token = OAuthTokenRecord {
            server_id: "oauth-test".to_string(),
            access_token: "super_secret_access_token".to_string(),
            refresh_token: Some("super_secret_refresh_token".to_string()),
            token_type: "Bearer".to_string(),
            expires_at: Some(1234567890),
            scope: Some("read write".to_string()),
            updated_at: 0,
        };

        registry.save_oauth_token(&token).await.unwrap();

        let retrieved = registry.get_oauth_token("oauth-test").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.access_token, "super_secret_access_token");
        assert_eq!(
            retrieved.refresh_token,
            Some("super_secret_refresh_token".to_string())
        );
    }
}
