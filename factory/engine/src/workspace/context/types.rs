//! ContextPack Types
//!
//! Defines the ContextPack structure for deterministic WIH rehydration.
//! Implements SYSTEM_LAW.md LAW-AUT-002 (Deterministic Rehydration Rule)

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// ContextPack seal structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackSeal {
    pub pack_id: String,
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub inputs_manifest: Vec<InputManifestEntry>,
    pub method_version: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle: Option<PolicyBundleRef>,
}

/// Input manifest entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputManifestEntry {
    pub path: String,
    pub hash: String,
    pub size_bytes: usize,
}

/// Policy bundle reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBundleRef {
    pub bundle_id: String,
    pub agents_md_hash: String,
    pub role_envelope: String,
    pub pack_ids: Vec<String>,
}

/// ContextPack inputs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackInputs {
    pub tier0_law: String,
    pub sot: String,
    pub architecture: String,
    pub contracts: Vec<ContractFile>,
    pub deltas: Vec<DeltaFile>,
    pub wih: WIH,
}

/// Contract file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractFile {
    pub path: String,
    pub content: String,
    pub hash: String,
}

/// Delta file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaFile {
    pub path: String,
    pub content: String,
    pub hash: String,
}

/// Work Item Header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WIH {
    pub wih_id: String,
    pub role: String,
    pub scope_paths: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub acceptance_refs: Vec<String>,
    pub execution_mode: String,
}

/// Request to seal a ContextPack
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealContextPackRequest {
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub inputs: ContextPackInputs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle: Option<PolicyBundleRef>,
}

/// Response from sealing a ContextPack
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealContextPackResponse {
    pub pack_id: String,
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub inputs_manifest: Vec<InputManifestEntry>,
    pub method_version: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle: Option<PolicyBundleRef>,
}

/// Query parameters for ContextPack retrieval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wih_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

/// Generate deterministic pack_id from inputs
pub fn generate_pack_id(inputs: &ContextPackInputs) -> String {
    // Collect all input hashes
    let mut hashes = Vec::new();
    hashes.push(sha256_hex(&inputs.tier0_law));
    hashes.push(sha256_hex(&inputs.sot));
    hashes.push(sha256_hex(&inputs.architecture));
    
    for contract in &inputs.contracts {
        hashes.push(contract.hash.replace("sha256:", ""));
    }
    
    for delta in &inputs.deltas {
        hashes.push(delta.hash.replace("sha256:", ""));
    }
    
    let wih_json = serde_json::to_string(&inputs.wih).unwrap_or_default();
    hashes.push(sha256_hex(&wih_json));
    
    // Sort hashes lexicographically (deterministic ordering)
    hashes.sort();
    
    // Concatenate and hash
    let concatenated = hashes.join("|");
    let pack_hash = sha256_hex(&concatenated);
    
    format!("cp_{}", pack_hash)
}

/// Compute SHA256 hash of content
fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Compute hash with prefix
pub fn sha256_with_prefix(content: &str) -> String {
    format!("sha256:{}", sha256_hex(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_pack_id() {
        let inputs = ContextPackInputs {
            tier0_law: "SYSTEM_LAW content".to_string(),
            sot: "SOT content".to_string(),
            architecture: "ARCH content".to_string(),
            contracts: vec![],
            deltas: vec![],
            wih: WIH {
                wih_id: "wih_123".to_string(),
                role: "builder".to_string(),
                scope_paths: vec!["/src".to_string()],
                allowed_tools: vec!["read".to_string()],
                acceptance_refs: vec![],
                execution_mode: "PLAN_ONLY".to_string(),
            },
        };
        
        let pack_id1 = generate_pack_id(&inputs);
        let pack_id2 = generate_pack_id(&inputs);
        
        assert_eq!(pack_id1, pack_id2, "Same inputs must produce same pack_id");
        assert!(pack_id1.starts_with("cp_"), "pack_id must start with cp_");
    }
}
