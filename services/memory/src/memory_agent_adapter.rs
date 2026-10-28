//! Memory Agent HTTP Adapter
//! 
//! Connects Rust services to the TypeScript Memory Agent via HTTP API
//! Implements the MemoryProvider trait

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn, error};

use allternit_memory_fabric::{
    MemoryProvider, MemoryEntry, MemoryQuery, MemoryError, MemoryCapabilities,
    QueryCapabilities, PerformanceCharacteristics, BackendType,
};

/// Configuration for the memory agent HTTP adapter
#[derive(Debug, Clone)]
pub struct MemoryAgentConfig {
    pub base_url: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for MemoryAgentConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:3201".to_string(),
            timeout_secs: 30,
            max_retries: 3,
        }
    }
}

/// HTTP adapter for the TypeScript Memory Agent
pub struct MemoryAgentAdapter {
    client: Client,
    config: MemoryAgentConfig,
}

/// API Request/Response structures
#[derive(Debug, Serialize)]
struct IngestRequest {
    content: String,
    source: Option<String>,
    metadata: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IngestResponse {
    success: bool,
    #[serde(default)]
    memory_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `/stats` response from the Memory Agent
#[derive(Debug, Clone, Deserialize)]
pub struct StatsResponse {
    pub memories: MemoryStats,
    pub insights: usize,
    pub connections: usize,
    #[serde(default)]
    pub processed_files: usize,
    #[serde(default)]
    pub ollama_connected: bool,
}

/// Memory counts by lifecycle stage
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryStats {
    pub total: usize,
    pub raw: usize,
    pub processed: usize,
    pub consolidated: usize,
    pub archived: usize,
}

impl MemoryAgentAdapter {
    /// Create a new memory agent adapter
    pub fn new(config: MemoryAgentConfig) -> Result<Self, MemoryError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| MemoryError::Connection(e.to_string()))?;

        Ok(Self { client, config })
    }

    /// Create with default configuration
    pub fn with_defaults() -> Result<Self, MemoryError> {
        Self::new(MemoryAgentConfig::default())
    }

    /// Create with custom base URL
    pub fn with_url(base_url: impl Into<String>) -> Result<Self, MemoryError> {
        let config = MemoryAgentConfig {
            base_url: base_url.into(),
            ..Default::default()
        };
        Self::new(config)
    }

    /// Health check
    pub async fn health(&self) -> Result<bool, MemoryError> {
        let url = format!("{}/health", self.config.base_url);
        
        match self.client.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                error!("Health check failed: {}", e);
                Err(MemoryError::Connection(e.to_string()))
            }
        }
    }

    /// Get the Memory Agent's typed statistics
    pub async fn agent_stats(&self) -> Result<StatsResponse, MemoryError> {
        let url = format!("{}/stats", self.config.base_url);
        
        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| MemoryError::Connection(e.to_string()))?;

        if !response.status().is_success() {
            return Err(MemoryError::Backend(format!(
                "Stats request failed: {}",
                response.status()
            )));
        }

        response
            .json::<StatsResponse>()
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))
    }

    /// Retry logic for requests
    async fn request_with_retry<T, F, Fut>(&self, request_fn: F) -> Result<T, MemoryError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, MemoryError>>,
    {
        let mut last_error = None;
        
        for attempt in 0..self.config.max_retries {
            match request_fn().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!("Request failed (attempt {}/{}): {}", attempt + 1, self.config.max_retries, e);
                    last_error = Some(e);
                    tokio::time::sleep(Duration::from_millis(100 * (attempt + 1) as u64)).await;
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| MemoryError::Backend("Unknown error".to_string())))
    }
}

#[async_trait]
impl MemoryProvider for MemoryAgentAdapter {
    /// Store a value in memory via HTTP API
    async fn store(&self, key: &str, value: Value) -> Result<(), MemoryError> {
        self.store_entry(MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            key: key.to_string(),
            value,
            tags: vec![],
            created_at: timestamp(),
            updated_at: timestamp(),
            expires_at: None,
            tenant_id: None,
            session_id: None,
            embedding: None,
        }).await
    }

    /// Store a complete memory entry
    async fn store_entry(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        let url = format!("{}/api/ingest", self.config.base_url);
        
        let request = IngestRequest {
            content: serde_json::to_string(&entry.value).unwrap_or_default(),
            source: Some(entry.key.clone()),
            metadata: None,
            tenant_id: entry.tenant_id,
            session_id: entry.session_id,
        };

        let response = self.request_with_retry(|| {
            let req = self.client.post(&url).json(&request);
            async move {
                req.send()
                    .await
                    .map_err(|e| MemoryError::Connection(e.to_string()))
            }
        }).await?;

        if !response.status().is_success() {
            return Err(MemoryError::Backend(format!(
                "Ingest request failed: {}",
                response.status()
            )));
        }

        let result: IngestResponse = response
            .json()
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?;

        if result.success {
            info!(
                "Stored memory entry: {} (id {})",
                entry.key,
                result.memory_id.as_deref().unwrap_or("unknown")
            );
            Ok(())
        } else {
            Err(MemoryError::Backend(
                result.error.unwrap_or_else(|| "Unknown ingest error".to_string())
            ))
        }
    }

    /// Retrieve a value by key (via search)
    async fn retrieve(&self, key: &str) -> Result<Option<Value>, MemoryError> {
        let entry = self.retrieve_entry(key).await?;
        Ok(entry.map(|e| e.value))
    }

    /// Retrieve a complete memory entry
    async fn retrieve_entry(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        // Search for the entry by key
        let query = MemoryQuery {
            query: key.to_string(),
            filters: HashMap::new(),
            limit: Some(1),
            offset: None,
            sort_by: None,
            ascending: None,
            min_similarity: None,
            tags: vec![],
            tenant_id: None,
            session_id: None,
        };

        let results = self.query(&query).await?;
        Ok(results.into_iter().next())
    }

    /// Query memory via HTTP API
    async fn query(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError> {
        let url = format!("{}/api/search", self.config.base_url);
        
        let limit = query.limit.unwrap_or(20).to_string();
        let response = self.request_with_retry(|| {
            let req = self
                .client
                .get(&url)
                .query(&[("q", query.query.as_str()), ("limit", limit.as_str())]);
            async move {
                req.send()
                    .await
                    .map_err(|e| MemoryError::Connection(e.to_string()))
            }
        }).await?;

        if !response.status().is_success() {
            return Err(MemoryError::Backend(format!(
                "Query request failed: {}",
                response.status()
            )));
        }

        #[derive(Debug, Deserialize)]
        struct SearchResponse {
            memories: Vec<MemoryEntry>,
        }

        let result: SearchResponse = response
            .json()
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?;

        Ok(result.memories)
    }

    /// Delete a memory entry
    async fn delete(&self, key: &str) -> Result<(), MemoryError> {
        // Find the memory entry first to get its ID
        let entries = self.query(&MemoryQuery {
            query: key.to_string(),
            filters: HashMap::new(),
            limit: Some(1),
            offset: None,
            sort_by: None,
            ascending: None,
            min_similarity: None,
            tags: vec![],
            tenant_id: None,
            session_id: None,
        }).await?;

        if let Some(entry) = entries.into_iter().next() {
            let url = format!("{}/api/memory/{}", self.config.base_url, entry.id);
            
            let response = self.client
                .delete(&url)
                .send()
                .await
                .map_err(|e| MemoryError::Connection(e.to_string()))?;

            if response.status() == StatusCode::NOT_FOUND {
                return Err(MemoryError::NotFound(format!("Memory not found: {}", key)));
            }

            if !response.status().is_success() {
                return Err(MemoryError::Backend(format!(
                    "Delete request failed: {}",
                    response.status()
                )));
            }
        }

        Ok(())
    }

    /// Check if a key exists
    async fn exists(&self, key: &str) -> Result<bool, MemoryError> {
        let entry = self.retrieve_entry(key).await?;
        Ok(entry.is_some())
    }

    /// Get the backend type
    fn backend_type(&self) -> BackendType {
        BackendType::MemoryAgentHttp
    }

    /// Get capabilities
    async fn capabilities_async(&self) -> MemoryCapabilities {
        MemoryCapabilities {
            persistent: true,
            supports_embeddings: true, // Via Ollama
            max_size_mb: None,
            ttl_support: false,
            query_capabilities: QueryCapabilities {
                supports_full_text_search: true,
                supports_vector_search: true, // Via embeddings
                supports_filters: true,
                max_results: Some(100),
            },
            performance_characteristics: PerformanceCharacteristics {
                avg_read_latency_ms: 100.0,
                avg_write_latency_ms: 200.0,
                throughput_ops_per_sec: 50,
            },
        }
    }

    /// Get statistics
    async fn stats(&self) -> Result<HashMap<String, Value>, MemoryError> {
        let stats = self.agent_stats().await?;
        
        let mut result = HashMap::new();
        result.insert("memories_total".to_string(), Value::Number(stats.memories.total.into()));
        result.insert("memories_raw".to_string(), Value::Number(stats.memories.raw.into()));
        result.insert("memories_processed".to_string(), Value::Number(stats.memories.processed.into()));
        result.insert("memories_consolidated".to_string(), Value::Number(stats.memories.consolidated.into()));
        result.insert("memories_archived".to_string(), Value::Number(stats.memories.archived.into()));
        result.insert("insights".to_string(), Value::Number(stats.insights.into()));
        result.insert("connections".to_string(), Value::Number(stats.connections.into()));
        result.insert("processed_files".to_string(), Value::Number(stats.processed_files.into()));
        result.insert("ollama_connected".to_string(), Value::Bool(stats.ollama_connected));
        
        Ok(result)
    }

    /// Ping the Memory Agent via its health endpoint
    async fn ping(&self) -> Result<bool, MemoryError> {
        self.health().await
    }
}

/// Get current timestamp in seconds
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = MemoryAgentConfig::default();
        assert_eq!(config.base_url, "http://127.0.0.1:3201");
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_config_with_url() {
        let config = MemoryAgentConfig {
            base_url: "http://localhost:3201".to_string(),
            ..Default::default()
        };
        assert_eq!(config.base_url, "http://localhost:3201");
    }
}
