pub mod memory_agent_adapter;

pub use memory_agent_adapter::{MemoryAgentAdapter, MemoryAgentConfig};

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use allternit_memory_fabric::{
    MemoryProvider, MemoryError, MemoryCapabilities, QueryCapabilities,
    PerformanceCharacteristics, BackendType, MemoryQuery, MemoryEntry
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub port: u16,
    pub host: String,
    pub storage_path: String,
}

pub struct MemoryService {
    config: MemoryConfig,
    memory_store: Arc<RwLock<std::collections::HashMap<String, serde_json::Value>>>,
}

impl MemoryService {
    pub fn new(config: MemoryConfig) -> Self {
        Self {
            config,
            memory_store: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting Memory Service on {}:{}", self.config.host, self.config.port);

        // TODO: Implement memory service functionality
        // This would typically handle memory persistence, recall, etc.

        Ok(())
    }

    pub async fn store_memory(&self, key: String, value: serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
        let mut store = self.memory_store.write().await;
        store.insert(key.clone(), value);
        debug!("Stored memory with key: {}", key);
        Ok(())
    }

    pub async fn retrieve_memory(&self, key: &str) -> Option<serde_json::Value> {
        let store = self.memory_store.read().await;
        store.get(key).cloned()
    }
}

#[async_trait]
impl MemoryProvider for MemoryService {
    async fn store(&self, key: &str, value: serde_json::Value) -> Result<(), MemoryError> {
        let mut store = self.memory_store.write().await;
        store.insert(key.to_string(), value);
        debug!("Stored memory with key: {}", key);
        Ok(())
    }

    async fn store_entry(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        let mut store = self.memory_store.write().await;
        store.insert(entry.key.clone(), entry.value);
        Ok(())
    }

    async fn retrieve(&self, key: &str) -> Result<Option<serde_json::Value>, MemoryError> {
        let store = self.memory_store.read().await;
        Ok(store.get(key).cloned())
    }

    async fn retrieve_entry(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let store = self.memory_store.read().await;
        if let Some(value) = store.get(key).cloned() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            Ok(Some(MemoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                key: key.to_string(),
                value,
                tags: vec![],
                created_at: now,
                updated_at: now,
                expires_at: None,
                tenant_id: None,
                session_id: None,
                embedding: None,
            }))
        } else {
            Ok(None)
        }
    }

    async fn query(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError> {
        let store = self.memory_store.read().await;
        let mut results = Vec::new();

        for (key, value) in store.iter() {
            // Basic filtering - in a real implementation, this would be more sophisticated
            if query.query.is_empty() || key.contains(&query.query) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                results.push(MemoryEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    key: key.clone(),
                    value: value.clone(),
                    tags: vec![],
                    created_at: now,
                    updated_at: now,
                    expires_at: None,
                    tenant_id: query.tenant_id.clone(),
                    session_id: query.session_id.clone(),
                    embedding: None,
                });
            }
        }

        // Apply limit if specified
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    async fn delete(&self, key: &str) -> Result<(), MemoryError> {
        let mut store = self.memory_store.write().await;
        store.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, MemoryError> {
        let store = self.memory_store.read().await;
        Ok(store.contains_key(key))
    }

    fn backend_type(&self) -> BackendType {
        BackendType::SimpleInMemory
    }

    async fn capabilities_async(&self) -> MemoryCapabilities {
        MemoryCapabilities {
            persistent: false,
            supports_embeddings: false,
            max_size_mb: None, // Limited by available RAM
            ttl_support: false,
            query_capabilities: QueryCapabilities {
                supports_full_text_search: true,
                supports_vector_search: false,
                supports_filters: false,
                max_results: None,
            },
            performance_characteristics: PerformanceCharacteristics {
                avg_read_latency_ms: 0.1,
                avg_write_latency_ms: 0.1,
                throughput_ops_per_sec: 100000,
            },
        }
    }

    async fn stats(&self) -> Result<std::collections::HashMap<String, serde_json::Value>, MemoryError> {
        let store = self.memory_store.read().await;
        let mut stats = std::collections::HashMap::new();
        stats.insert("entry_count".to_string(), serde_json::Value::Number(serde_json::Number::from(store.len())));
        stats.insert("backend_type".to_string(), serde_json::Value::String("simple_in_memory".to_string()));
        Ok(stats)
    }

    async fn ping(&self) -> Result<bool, MemoryError> {
        Ok(true) // Always available since it's in-memory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_service_creation() {
        let config = MemoryConfig {
            port: 3008,
            host: "localhost".to_string(),
            storage_path: "/tmp/memory-test".to_string(),
        };
        
        let service = MemoryService::new(config);
        assert_eq!(service.config.port, 3008);
    }
    
    #[tokio::test]
    async fn test_memory_store_retrieve() {
        let config = MemoryConfig {
            port: 3008,
            host: "localhost".to_string(),
            storage_path: "/tmp/memory-test".to_string(),
        };
        
        let service = MemoryService::new(config);
        let test_key = "test-key".to_string();
        let test_value = serde_json::json!({"test": "data"});
        
        service.store_memory(test_key.clone(), test_value.clone()).await.unwrap();
        let retrieved = service.retrieve_memory(&test_key).await;
        
        assert_eq!(retrieved, Some(test_value));
    }
}
