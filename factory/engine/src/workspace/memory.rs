//! Persistent memory for the Rails CLI.
//!
//! Memories are long-lived facts, conventions, or project context that
//! agents can recall and inject into their session context.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};

/// Default directory for memories, relative to workspace root.
pub const MEMORY_DIR: &str = ".allternit/rails/memories";

/// A persistent memory entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub accessed_at: Option<DateTime<Utc>>,
    pub access_count: u64,
}

/// Patch for updating a memory.
#[derive(Clone, Debug, Default)]
pub struct MemoryUpdate {
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Event-sourced memory store.
pub struct MemoryStore {
    memories_dir: PathBuf,
}

impl MemoryStore {
    /// Open the store rooted at `root`.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let memories_dir = root.join(MEMORY_DIR);
        ensure_dir(&memories_dir)?;
        Ok(Self { memories_dir })
    }

    /// Store a new memory and return it.
    pub fn learn(
        &self,
        content: impl Into<String>,
        tags: Vec<String>,
    ) -> Result<Memory> {
        let content = content.into();
        let mut nonce = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut nonce);
        let id = format!("mem-{}-{}", Utc::now().timestamp_millis(), hex::encode(nonce));
        let memory = Memory {
            id: id.clone(),
            content,
            tags,
            created_at: Utc::now(),
            accessed_at: None,
            access_count: 0,
        };
        self.write(&memory)?;
        Ok(memory)
    }

    /// Retrieve a memory by ID, bumping its access counter.
    pub fn recall(&self, id: &str) -> Result<Option<Memory>> {
        let mut memory: Option<Memory> = read_json(&self.path(id))?;
        if let Some(ref mut m) = memory {
            m.accessed_at = Some(Utc::now());
            m.access_count += 1;
            self.write(m)?;
        }
        Ok(memory)
    }

    /// Update an existing memory.
    pub fn update(&self, id: &str, patch: MemoryUpdate) -> Result<Memory> {
        let mut memory = self
            .recall(id)?
            .with_context(|| format!("memory {id} not found"))?;
        if let Some(content) = patch.content {
            memory.content = content;
        }
        if let Some(tags) = patch.tags {
            memory.tags = tags;
        }
        memory.accessed_at = Some(Utc::now());
        self.write(&memory)?;
        Ok(memory)
    }

    /// Delete a memory.
    pub fn forget(&self, id: &str) -> Result<bool> {
        let path = self.path(id);
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all memories, optionally filtered by tag.
    pub fn list(&self, tag: Option<&str>) -> Result<Vec<Memory>> {
        let mut memories = Vec::new();
        for entry in std::fs::read_dir(&self.memories_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(memory) = read_json::<Memory>(&entry.path())? {
                    if let Some(tag) = tag {
                        if !memory.tags.contains(&tag.to_string()) {
                            continue;
                        }
                    }
                    memories.push(memory);
                }
            }
        }
        memories.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(memories)
    }

    /// Search memories by substring in content or tags.
    pub fn search(&self, query: &str) -> Result<Vec<Memory>> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();
        for memory in self.list(None)? {
            if memory.content.to_lowercase().contains(&query_lower)
                || memory.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
            {
                results.push(memory);
            }
        }
        Ok(results)
    }

    /// Build a brief context string from memories matching the given tags.
    ///
    /// If `tags` is empty, all memories are included. Memories are sorted by
    /// access count (most used first) and then recency.
    pub fn brief(&self, tags: &[String], limit: usize) -> Result<String> {
        let mut memories = self.list(None)?;
        if !tags.is_empty() {
            let tag_set: HashSet<_> = tags.iter().cloned().collect();
            memories.retain(|m| m.tags.iter().any(|t| tag_set.contains(t)));
        }
        memories.sort_by(|a, b| {
            b.access_count
                .cmp(&a.access_count)
                .then_with(|| b.accessed_at.cmp(&a.accessed_at))
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        memories.truncate(limit);

        if memories.is_empty() {
            return Ok(String::new());
        }

        let mut out = String::from("# Rails Memory Brief\n\n");
        for memory in memories {
            out.push_str(&format!("## {}\n\n{}\n\n", memory.id, memory.content));
        }
        Ok(out)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.memories_dir.join(format!("{}.json", id))
    }

    fn write(&self, memory: &Memory) -> Result<()> {
        let path = self.path(&memory.id);
        write_json_atomic(&path, memory)
            .with_context(|| format!("failed to write memory {path:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn learn_and_recall() {
        let tmp = TempDir::new().unwrap();
        let store = MemoryStore::new(tmp.path()).unwrap();
        let mem = store.learn("Use hash-based IDs", vec!["id".to_string()]).unwrap();

        let recalled = store.recall(&mem.id).unwrap().unwrap();
        assert_eq!(recalled.content, "Use hash-based IDs");
        assert_eq!(recalled.access_count, 1);
    }

    #[test]
    fn forget_memory() {
        let tmp = TempDir::new().unwrap();
        let store = MemoryStore::new(tmp.path()).unwrap();
        let mem = store.learn("Temp", vec![]).unwrap();
        assert!(store.forget(&mem.id).unwrap());
        assert!(store.recall(&mem.id).unwrap().is_none());
    }

    #[test]
    fn search_and_brief() {
        let tmp = TempDir::new().unwrap();
        let store = MemoryStore::new(tmp.path()).unwrap();
        store
            .learn("Always use ledger-first storage", vec!["storage".to_string()])
            .unwrap();
        store
            .learn("Dolt is optional", vec!["storage".to_string(), "dolt".to_string()])
            .unwrap();

        let results = store.search("ledger").unwrap();
        assert_eq!(results.len(), 1);

        let brief = store.brief(&["storage".to_string()], 10).unwrap();
        assert!(brief.contains("ledger-first"));
        assert!(brief.contains("Dolt is optional"));
    }
}
