//! Dynamic Hugging Face model catalog.
//!
//! Merges a small in-repo seed list of newest models with models polled from
//! Hugging Face's public API. Results are cached on disk with a TTL so the
//! service stays responsive even when HF is slow or rate-limited.

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// A single catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub repo_id: String,
    pub downloads: u64,
    pub likes: u64,
    pub tags: Vec<String>,
    pub pipeline_tag: Option<String>,
    pub last_modified: Option<String>,
    pub source: String,
}

impl CatalogEntry {
    pub fn from_seed(seed: &SeedEntry) -> Self {
        Self {
            repo_id: seed.repo_id.clone(),
            downloads: 0,
            likes: 0,
            tags: vec!["gguf".to_string()],
            pipeline_tag: Some("text-generation".to_string()),
            last_modified: None,
            source: "seed".to_string(),
        }
    }
}

/// In-repo seed entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedEntry {
    pub repo_id: String,
    pub source_tag: String,
}

/// Cached poll result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolledCache {
    pub polled_at: DateTime<Utc>,
    pub entries: Vec<CatalogEntry>,
}

/// Catalog service configuration.
#[derive(Debug, Clone)]
pub struct CatalogConfig {
    pub poll_interval: std::time::Duration,
    pub cache_ttl: std::time::Duration,
    pub poll_limit: usize,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(24 * 60 * 60),
            cache_ttl: std::time::Duration::from_secs(24 * 60 * 60),
            poll_limit: 20,
        }
    }
}

/// Shared catalog service.
#[derive(Clone)]
pub struct CatalogService {
    data_dir: PathBuf,
    config: CatalogConfig,
    client: Client,
    seeds: Vec<SeedEntry>,
    cache: Arc<RwLock<PolledCache>>,
    /// Set while a stale-while-revalidate refresh is running, so concurrent
    /// `/catalog` requests do not stack duplicate polls.
    refreshing: Arc<AtomicBool>,
}

/// How old the cached poll may be before `/catalog` triggers a revalidate.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

impl CatalogService {
    /// Create a new catalog service, loading seed entries and any cached poll.
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref().to_path_buf();
        let seeds = load_seeds();
        let cache = load_cache(&data_dir).unwrap_or_else(|| PolledCache {
            polled_at: DateTime::UNIX_EPOCH,
            entries: vec![],
        });

        Self {
            data_dir,
            config: CatalogConfig::default(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            seeds,
            cache: Arc::new(RwLock::new(cache)),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Override the default config (useful for tests).
    pub fn with_config(mut self, config: CatalogConfig) -> Self {
        self.config = config;
        self
    }

    /// Return the merged catalog.
    pub async fn catalog(&self, source: CatalogSource, limit: usize) -> Vec<CatalogEntry> {
        let polled = self.cache.read().await.entries.clone();
        let mut merged: Vec<CatalogEntry> = match source {
            CatalogSource::All => {
                let mut by_id: HashMap<String, CatalogEntry> = HashMap::new();
                for entry in &polled {
                    by_id.insert(entry.repo_id.clone(), entry.clone());
                }
                for seed in &self.seeds {
                    by_id
                        .entry(seed.repo_id.clone())
                        .or_insert_with(|| CatalogEntry::from_seed(seed));
                }
                by_id.into_values().collect()
            }
            CatalogSource::Polled => polled,
            CatalogSource::Seed => self.seeds.iter().map(CatalogEntry::from_seed).collect(),
        };

        merged.sort_by(|a, b| b.downloads.cmp(&a.downloads));
        merged.truncate(limit);
        merged
    }

    /// Force a refresh from Hugging Face and return the number of polled entries.
    pub async fn refresh(&self) -> Result<usize, CatalogError> {
        // If a stale-while-revalidate refresh is in flight, give it up to a
        // minute to finish instead of stacking a duplicate HF poll on top.
        for _ in 0..120 {
            if !self.refreshing.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let result = self.refresh_locked().await;
        self.refreshing.store(false, Ordering::SeqCst);
        result
    }

    /// Epoch seconds of the last successful HF poll, or `None` if the cache
    /// has never been populated (neither from disk nor from the network).
    pub async fn fetched_at(&self) -> Option<u64> {
        let polled_at = self.cache.read().await.polled_at;
        if polled_at == DateTime::UNIX_EPOCH {
            None
        } else {
            Some(polled_at.timestamp() as u64)
        }
    }

    /// Whether the cached poll is missing or older than `max_age`.
    pub async fn is_stale(&self, max_age: Duration) -> bool {
        let polled_at = self.cache.read().await.polled_at;
        polled_at == DateTime::UNIX_EPOCH
            || (Utc::now() - polled_at)
                .to_std()
                .map(|age| age > max_age)
                .unwrap_or(true)
    }

    /// Stale-while-revalidate entry point used by `GET /catalog`: when the
    /// cache is stale and no refresh is currently in flight, spawn a
    /// background refresh and return `true`. The caller serves the current
    /// cache regardless. Poll failures leave the last good cache untouched.
    pub async fn spawn_refresh_if_stale(&self, max_age: Duration) -> bool {
        if !self.is_stale(max_age).await {
            return false;
        }
        if self
            .refreshing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        let service = self.clone();
        tokio::spawn(async move {
            let result = service.refresh_locked().await;
            service.refreshing.store(false, Ordering::SeqCst);
            match result {
                Ok(count) => info!(count, "stale catalog refreshed in background"),
                Err(err) => {
                    warn!(error = %err, "background catalog refresh failed; serving last good cache")
                }
            }
        });
        true
    }

    async fn refresh_locked(&self) -> Result<usize, CatalogError> {
        let entries = self.poll_huggingface().await?;
        let count = entries.len();
        let cache = PolledCache {
            polled_at: Utc::now(),
            entries,
        };
        let _ = save_cache(&self.data_dir, &cache);
        *self.cache.write().await = cache;
        info!(count, "catalog refreshed from Hugging Face");
        Ok(count)
    }

    /// Start background polling.
    pub fn spawn_background_refresh(self) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.config.poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(err) = self.refresh().await {
                    warn!(error = %err, "background catalog refresh failed");
                }
            }
        });
    }

    /// Poll HF for top GGUF models by downloads and likes.
    async fn poll_huggingface(&self) -> Result<Vec<CatalogEntry>, CatalogError> {
        let mut all = Vec::new();
        for sort in ["downloads", "likes"] {
            let url = format!(
                "https://huggingface.co/api/models?filter=gguf&sort={}&direction=-1&limit={}",
                sort, self.config.poll_limit
            );
            let res = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(CatalogError::Http)?;
            if !res.status().is_success() {
                warn!(status = %res.status(), "Hugging Face API returned non-success");
                continue;
            }
            let rows: Vec<serde_json::Value> = res.json().await.map_err(CatalogError::Http)?;
            for row in rows {
                let repo_id = row["id"]
                    .as_str()
                    .or_else(|| row["modelId"].as_str())
                    .map(|s| s.to_string());
                let Some(repo_id) = repo_id else { continue };
                let downloads = row["downloads"].as_u64().unwrap_or(0);
                let likes = row["likes"].as_u64().unwrap_or(0);
                let tags = row["tags"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let pipeline_tag = row["pipeline_tag"].as_str().map(|s| s.to_string());
                let last_modified = row["lastModified"].as_str().map(|s| s.to_string());

                all.push(CatalogEntry {
                    repo_id,
                    downloads,
                    likes,
                    tags,
                    pipeline_tag,
                    last_modified,
                    source: "polled".to_string(),
                });
            }
        }

        // Deduplicate, keeping the entry with the highest downloads.
        let mut by_id: HashMap<String, CatalogEntry> = HashMap::new();
        for entry in all {
            by_id
                .entry(entry.repo_id.clone())
                .and_modify(|e| {
                    if entry.downloads > e.downloads {
                        *e = entry.clone();
                    }
                })
                .or_insert(entry);
        }

        let mut entries: Vec<CatalogEntry> = by_id.into_values().collect();
        entries.sort_by(|a, b| b.downloads.cmp(&a.downloads));
        Ok(entries)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CatalogSource {
    All,
    Polled,
    Seed,
}

impl CatalogSource {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "polled" => CatalogSource::Polled,
            "seed" => CatalogSource::Seed,
            _ => CatalogSource::All,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

fn load_seeds() -> Vec<SeedEntry> {
    // Embed the repo-relative seed file at compile time; fall back to a
    // compiled default list if the file cannot be parsed.
    const SEED_JSON: &str = include_str!("../../catalog_seed.json");
    serde_json::from_str(SEED_JSON).unwrap_or_else(|_| {
        vec![
            SeedEntry {
                repo_id: "bartowski/Llama-3.2-3B-Instruct-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/Llama-3.2-1B-Instruct-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/Qwen2.5-7B-Instruct-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/Qwen2.5-14B-Instruct-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/Mistral-7B-Instruct-v0.3-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/DeepSeek-R1-Distill-Qwen-7B-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/Phi-4-mini-instruct-GGUF".into(),
                source_tag: "newest".into(),
            },
            SeedEntry {
                repo_id: "bartowski/gemma-2-9b-it-GGUF".into(),
                source_tag: "newest".into(),
            },
        ]
    })
}

fn cache_path(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("catalog_cache.json")
}

fn load_cache(data_dir: impl AsRef<Path>) -> Option<PolledCache> {
    let path = cache_path(data_dir);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_cache(data_dir: impl AsRef<Path>, cache: &PolledCache) -> Result<(), CatalogError> {
    let path = cache_path(data_dir);
    let json = serde_json::to_string_pretty(cache)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_loads() {
        let seeds = load_seeds();
        assert!(!seeds.is_empty());
    }

    #[test]
    fn catalog_source_parsing() {
        assert!(matches!(
            CatalogSource::from_str("polled"),
            CatalogSource::Polled
        ));
        assert!(matches!(CatalogSource::from_str("all"), CatalogSource::All));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn staleness_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let service = CatalogService::new(dir.path());

        // Never polled: no fetched_at, treated as stale.
        assert_eq!(service.fetched_at().await, None);
        assert!(service.is_stale(STALE_AFTER).await);

        // Fresh in-memory poll: fetched_at present, not stale.
        {
            let mut cache = service.cache.write().await;
            cache.polled_at = Utc::now();
        }
        assert!(service.fetched_at().await.is_some());
        assert!(!service.is_stale(STALE_AFTER).await);

        // Age beyond the threshold: stale again.
        {
            let mut cache = service.cache.write().await;
            cache.polled_at = Utc::now() - chrono::Duration::hours(2);
        }
        assert!(service.is_stale(STALE_AFTER).await);
        assert!(!service.is_stale(Duration::from_secs(24 * 60 * 60)).await);
    }
}
