//! ContextPack Store
//!
//! Handles persistence and retrieval of sealed ContextPacks.

use std::path::PathBuf;

use crate::core::io::{ensure_dir, write_json_atomic};
use anyhow::Result;

use super::types::{ContextPackSeal, InputManifestEntry};

#[derive(Debug, Clone)]
pub struct ContextPackStoreOptions {
    pub root_dir: Option<PathBuf>,
    pub context_packs_dir: Option<PathBuf>,
}

pub struct ContextPackStore {
    context_packs_dir: PathBuf,
}

impl ContextPackStore {
    pub fn new(opts: ContextPackStoreOptions) -> Result<Self> {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let context_packs_dir = opts
            .context_packs_dir
            .unwrap_or_else(|| PathBuf::from(".allternit/context-packs"));

        let context_packs_dir = if context_packs_dir.is_absolute() {
            context_packs_dir
        } else {
            root_dir.join(context_packs_dir)
        };

        ensure_dir(&context_packs_dir)?;

        Ok(Self {
            context_packs_dir,
        })
    }

    /// Store a sealed ContextPack
    pub fn store_seal(&self, seal: &ContextPackSeal) -> Result<PathBuf> {
        let pack_dir = self.context_packs_dir.join(&seal.pack_id);
        ensure_dir(&pack_dir)?;
        
        let seal_path = pack_dir.join("seal.json");
        write_json_atomic(&seal_path, seal)?;
        
        Ok(seal_path)
    }

    /// Retrieve a ContextPack seal by pack_id
    pub fn get_seal(&self, pack_id: &str) -> Result<Option<ContextPackSeal>> {
        let pack_dir = self.context_packs_dir.join(pack_id);
        let seal_path = pack_dir.join("seal.json");
        
        if !seal_path.exists() {
            return Ok(None);
        }
        
        let seal_content = std::fs::read_to_string(&seal_path)?;
        let seal: ContextPackSeal = serde_json::from_str(&seal_content)?;
        Ok(Some(seal))
    }

    /// Query ContextPacks by WIH ID
    pub fn query_by_wih(&self, wih_id: &str) -> Result<Vec<ContextPackSeal>> {
        let mut results = Vec::new();
        
        if !self.context_packs_dir.exists() {
            return Ok(results);
        }
        
        for entry in std::fs::read_dir(&self.context_packs_dir)? {
            let entry = entry?;
            let pack_dir = entry.path();
            
            if !pack_dir.is_dir() {
                continue;
            }
            
            let seal_path = pack_dir.join("seal.json");
            if !seal_path.exists() {
                continue;
            }
            
            let seal_content = std::fs::read_to_string(&seal_path)?;
            let seal: ContextPackSeal = serde_json::from_str(&seal_content)?;
            
            if seal.wih_id == wih_id {
                results.push(seal);
            }
        }
        
        Ok(results)
    }

    /// Query ContextPacks by DAG ID
    pub fn query_by_dag(&self, dag_id: &str) -> Result<Vec<ContextPackSeal>> {
        let mut results = Vec::new();
        
        if !self.context_packs_dir.exists() {
            return Ok(results);
        }
        
        for entry in std::fs::read_dir(&self.context_packs_dir)? {
            let entry = entry?;
            let pack_dir = entry.path();
            
            if !pack_dir.is_dir() {
                continue;
            }
            
            let seal_path = pack_dir.join("seal.json");
            if !seal_path.exists() {
                continue;
            }
            
            let seal_content = std::fs::read_to_string(&seal_path)?;
            let seal: ContextPackSeal = serde_json::from_str(&seal_content)?;
            
            if seal.dag_id == dag_id {
                results.push(seal);
            }
        }
        
        Ok(results)
    }

    /// Get the seal path for a pack_id
    pub fn seal_path(&self, pack_id: &str) -> PathBuf {
        self.context_packs_dir.join(pack_id).join("seal.json")
    }

    /// Store input manifest
    pub fn store_manifest(&self, pack_id: &str, manifest: &[InputManifestEntry]) -> Result<PathBuf> {
        let pack_dir = self.context_packs_dir.join(pack_id);
        ensure_dir(&pack_dir)?;

        let manifest_path = pack_dir.join("inputs_manifest.json");
        // Wrap slice in a struct for serialization
        let manifest_wrapper = serde_json::json!({ "entries": manifest });
        write_json_atomic(&manifest_path, &manifest_wrapper)?;

        Ok(manifest_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, ContextPackStore) {
        let temp_dir = TempDir::new().unwrap();
        let store = ContextPackStore::new(ContextPackStoreOptions {
            root_dir: Some(temp_dir.path().to_path_buf()),
            context_packs_dir: Some(PathBuf::from(".allternit/context-packs")),
        }).unwrap();
        (temp_dir, store)
    }

    #[test]
    fn test_store_and_retrieve_seal() {
        let (_temp_dir, store) = create_test_store();
        
        let seal = ContextPackSeal {
            pack_id: "cp_test123".to_string(),
            wih_id: "wih_abc".to_string(),
            dag_id: "dag_xyz".to_string(),
            node_id: "node_001".to_string(),
            inputs_manifest: vec![],
            method_version: "1.0.0".to_string(),
            created_at: Utc::now().to_rfc3339(),
            policy_bundle: None,
        };
        
        store.store_seal(&seal).unwrap();
        let retrieved = store.get_seal("cp_test123").unwrap();
        
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.pack_id, seal.pack_id);
        assert_eq!(retrieved.wih_id, seal.wih_id);
    }

    #[test]
    fn test_query_by_wih() {
        let (_temp_dir, store) = create_test_store();
        
        let seal1 = ContextPackSeal {
            pack_id: "cp_test1".to_string(),
            wih_id: "wih_abc".to_string(),
            dag_id: "dag_xyz".to_string(),
            node_id: "node_001".to_string(),
            inputs_manifest: vec![],
            method_version: "1.0.0".to_string(),
            created_at: Utc::now().to_rfc3339(),
            policy_bundle: None,
        };
        
        let seal2 = ContextPackSeal {
            pack_id: "cp_test2".to_string(),
            wih_id: "wih_abc".to_string(),
            dag_id: "dag_xyz".to_string(),
            node_id: "node_002".to_string(),
            inputs_manifest: vec![],
            method_version: "1.0.0".to_string(),
            created_at: Utc::now().to_rfc3339(),
            policy_bundle: None,
        };
        
        store.store_seal(&seal1).unwrap();
        store.store_seal(&seal2).unwrap();
        
        let results = store.query_by_wih("wih_abc").unwrap();
        assert_eq!(results.len(), 2);
    }
}
