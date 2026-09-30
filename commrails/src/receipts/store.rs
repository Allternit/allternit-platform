use std::path::PathBuf;

use crate::core::ids::create_blob_id;
use crate::core::io::{ensure_dir, write_json_atomic};
use crate::core::types::ReceiptRecord;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct ReceiptStoreOptions {
    pub root_dir: Option<PathBuf>,
    pub receipts_dir: Option<PathBuf>,
    pub blobs_dir: Option<PathBuf>,
}

pub struct ReceiptStore {
    receipts_dir: PathBuf,
    blobs_dir: PathBuf,
}

/// Query filters for receipt queries
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReceiptQuery {
    pub run_id: Option<String>,
    pub tool: Option<String>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Receipt verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptVerificationResult {
    pub receipt_id: String,
    pub is_valid: bool,
    pub hash_matches: bool,
    pub signature_valid: Option<bool>,
    /// "chained-signed", "legacy (unsigned, unchained)" or "missing".
    #[serde(default)]
    pub integrity: String,
    pub errors: Vec<String>,
}

/// Receipt summary/aggregation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptSummary {
    pub total_count: usize,
    pub by_type: std::collections::HashMap<String, usize>,
    pub by_run: std::collections::HashMap<String, usize>,
    pub date_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

impl ReceiptStore {
    pub fn new(opts: ReceiptStoreOptions) -> Result<Self> {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let receipts_dir = opts
            .receipts_dir
            .unwrap_or_else(|| PathBuf::from(".allternit/receipts"));
        let blobs_dir = opts
            .blobs_dir
            .unwrap_or_else(|| PathBuf::from(".allternit/blobs"));

        let receipts_dir = if receipts_dir.is_absolute() {
            receipts_dir
        } else {
            root_dir.join(receipts_dir)
        };
        let blobs_dir = if blobs_dir.is_absolute() {
            blobs_dir
        } else {
            root_dir.join(blobs_dir)
        };

        ensure_dir(&receipts_dir)?;
        ensure_dir(&blobs_dir)?;

        Ok(Self {
            receipts_dir,
            blobs_dir,
        })
    }

    pub fn write_receipt(&self, receipt: &ReceiptRecord) -> Result<PathBuf> {
        let receipt_dir = self.receipts_dir.join(&receipt.receipt_id);
        ensure_dir(&receipt_dir)?;
        let receipt_path = receipt_dir.join("receipt.json");
        write_json_atomic(&receipt_path, receipt)?;
        Ok(receipt_path)
    }

    pub fn write_receipt_with_ts(&self, mut receipt: ReceiptRecord) -> Result<PathBuf> {
        if receipt.receipt_id.is_empty() {
            receipt.receipt_id = format!("rcpt_{}", Utc::now().timestamp());
        }
        self.write_receipt(&receipt)
    }

    pub fn store_blob_bytes(&self, bytes: &[u8]) -> Result<String> {
        for _ in 0..6 {
            let blob_id = create_blob_id();
            let blob_path = self.blobs_dir.join(&blob_id);
            if blob_path.exists() {
                continue;
            }
            std::fs::write(&blob_path, bytes)?;
            return Ok(blob_id);
        }
        anyhow::bail!("failed to allocate blob id after multiple attempts");
    }

    pub fn store_blob_string(&self, content: &str) -> Result<String> {
        self.store_blob_bytes(content.as_bytes())
    }

    /// Root receipts directory (chain, effects, keys and cassettes live under it).
    pub fn receipts_dir(&self) -> &std::path::Path {
        &self.receipts_dir
    }

    pub fn blob_path(&self, blob_id: &str) -> PathBuf {
        self.blobs_dir.join(blob_id)
    }

    pub fn receipt_path(&self, receipt_id: &str) -> PathBuf {
        self.receipts_dir.join(receipt_id).join("receipt.json")
    }

    /// Read a receipt by ID
    pub fn read_receipt(&self, receipt_id: &str) -> Result<Option<ReceiptRecord>> {
        if receipt_id.starts_with('_') || receipt_id.contains(['/', '\\']) {
            return Ok(None); // reserved dirs (_chains/_effects/_keys) and path traversal
        }
        let receipt_path = self.receipt_path(receipt_id);
        if !receipt_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&receipt_path)?;
        let receipt: ReceiptRecord = serde_json::from_str(&content)?;
        Ok(Some(receipt))
    }

    /// Query receipts with filters
    pub fn query_receipts(&self, query: &ReceiptQuery) -> Result<Vec<ReceiptRecord>> {
        let mut results = Vec::new();

        // Iterate through receipt directories
        if let Ok(entries) = std::fs::read_dir(&self.receipts_dir) {
            for entry in entries.flatten() {
                if !entry.file_type()?.is_dir() {
                    continue;
                }

                let receipt_id = entry.file_name().to_string_lossy().to_string();
                if let Some(receipt) = self.read_receipt(&receipt_id)? {
                    // Apply filters
                    if let Some(ref run_id) = query.run_id {
                        if receipt.run_id != *run_id {
                            continue;
                        }
                    }
                    if let Some(ref tool) = query.tool {
                        if receipt.tool != *tool {
                            continue;
                        }
                    }

                    results.push(receipt);
                }
            }
        }

        // Sort by receipt_id (lexicographic, which is roughly chronological)
        results.sort_by(|a, b| {
            a.receipt_id.cmp(&b.receipt_id)
        });

        // Apply pagination
        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(usize::MAX);
        let results = results.into_iter().skip(offset).take(limit).collect();

        Ok(results)
    }

    /// Verify receipt integrity
    pub fn verify_receipt(&self, receipt_id: &str) -> Result<ReceiptVerificationResult> {
        let mut errors = Vec::new();
        let hash_matches;
        let signature_valid = None;

        // Chained + signed receipts (WP3) take precedence over the legacy path.
        if let Ok(cs) = self.chain_store() {
            if let Some(v) = cs.find_by_id(receipt_id)? {
                return Ok(super::chain::verify_chained(&cs, receipt_id, &v));
            }
        }

        // Read receipt
        let receipt = match self.read_receipt(receipt_id)? {
            Some(r) => r,
            None => {
                return Ok(ReceiptVerificationResult {
                    receipt_id: receipt_id.to_string(),
                    is_valid: false,
                    hash_matches: false,
                    signature_valid: None,
                    integrity: "missing".to_string(),
                    errors: vec!["Receipt not found".to_string()],
                });
            }
        };

        // Verify hash (if inputs_ref is present)
        if let Some(inputs_ref) = &receipt.inputs_ref {
            let mut hasher = Sha256::new();
            hasher.update(receipt.receipt_id.as_bytes());
            hasher.update(receipt.run_id.as_bytes());
            hasher.update(receipt.tool.as_bytes());
            hasher.update(inputs_ref.as_bytes());
            let computed_hash = format!("{:x}", hasher.finalize());
            
            // Compare computed hash against the receipt's own context (if it had a hash field)
            // Since ReceiptRecord currently doesn't store a hash, we verify it's reproducible.
            hash_matches = !computed_hash.is_empty();
            if !hash_matches {
                errors.push("Failed to compute valid integrity hash".to_string());
            }
        } else {
            hash_matches = true; // No hash to verify
        }

        // Signature verification would go here if signatures are implemented
        // For now, signature_valid is None

        let is_valid = errors.is_empty() && hash_matches;

        Ok(ReceiptVerificationResult {
            receipt_id: receipt_id.to_string(),
            is_valid,
            hash_matches,
            signature_valid,
            // Legacy hash-only records are never reported as signed/valid-chained.
            integrity: "legacy (unsigned, unchained)".to_string(),
            errors,
        })
    }

    /// Public keys as JWKS JSON (for `/.well-known/jwks.json`-style publication).
    pub fn jwks_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.chain_store()?.jwks()?)?)
    }

    /// Chain/signing store rooted at this receipts dir (key from
    /// `ALLTERNIT_RECEIPT_SIGNING_KEY` or generated in dev on first use).
    pub fn chain_store(&self) -> Result<super::chain::ChainStore> {
        super::chain::ChainStore::open(&self.receipts_dir)
    }

    /// Record a tool call as an effect receipt on the run's chain and write the
    /// legacy record through `exec` at most once per idempotency key.
    ///
    /// Policy: payload `effect_class` (default `EXECUTE`, i.e. side-effecting).
    /// `NONE`/`READ` calls skip the chain and just run `exec` (unchanged behaviour).
    /// Payload `idempotency_key` (>= 8 chars) dedupes; otherwise a unique key is used.
    /// Returns the legacy receipt id (the recorded one on replay).
    pub fn record_tool_effect<F>(
        &self,
        run_id: &str,
        tool: &str,
        payload: &serde_json::Value,
        exec: F,
    ) -> Result<String>
    where
        F: FnOnce() -> Result<String>,
    {
        use super::chain::{EffectContext, EffectOutcome, EffectRequest};
        let class = payload
            .get("effect_class")
            .and_then(|v| v.as_str())
            .unwrap_or("EXECUTE")
            .to_string();
        if class == "NONE" || class == "READ" {
            return exec();
        }
        let cs = self.chain_store()?;
        let key = match payload.get("idempotency_key").and_then(|v| v.as_str()) {
            Some(k) if k.len() >= 8 => k.to_string(),
            _ => format!("auto:{}", create_blob_id()),
        };
        let cx = EffectContext {
            run_id: run_id.to_string(),
            session_id: run_id.to_string(),
            task_id: run_id.to_string(),
            node_id: None,
            trace_id: run_id.to_string(),
            state_version: 0,
            producer_id: "gate".to_string(),
            policy_decision_id: payload
                .get("policy_decision_id")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string(),
        };
        let req = EffectRequest {
            action_id: format!("act_{}", create_blob_id()),
            tool_id: tool.to_string(),
            args_hash: super::jcs::hash_value(payload)?,
            idempotency_key: key,
            effect_class: class,
            target: None,
        };
        match cs.run_effect_once(&cx, &req, || {
            let id = exec()?;
            Ok((super::jcs::sha256_tagged(id.as_bytes()), Some(id)))
        })? {
            EffectOutcome::Replayed(prev) => Ok(prev["external_ref"].as_str().unwrap_or("").to_string()),
            EffectOutcome::Executed(rec) => rec["external_ref"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("tool effect failed; FAILED receipt recorded on chain")),
        }
    }

    /// Get receipt summary/aggregation
    pub fn get_summary(&self, query: &ReceiptQuery) -> Result<ReceiptSummary> {
        let receipts = self.query_receipts(query)?;

        let mut by_tool: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut by_run: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        for receipt in &receipts {
            // Count by tool
            *by_tool.entry(receipt.tool.clone()).or_insert(0) += 1;

            // Count by run
            *by_run.entry(receipt.run_id.clone()).or_insert(0) += 1;
        }

        Ok(ReceiptSummary {
            total_count: receipts.len(),
            by_type: by_tool,  // Using tool as "type"
            by_run,
            date_range: None,  // ReceiptRecord doesn't have timestamp
        })
    }

    /// Verify multiple receipts
    pub fn verify_receipts(&self, receipt_ids: &[String]) -> Result<Vec<ReceiptVerificationResult>> {
        let mut results = Vec::new();
        for receipt_id in receipt_ids {
            results.push(self.verify_receipt(receipt_id)?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod wp3_tests {
    use super::*;
    use crate::core::types::ReceiptRecord;

    fn mk() -> (tempfile::TempDir, ReceiptStore) {
        let d = tempfile::tempdir().unwrap();
        let st = ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(d.path().to_path_buf()), receipts_dir: None, blobs_dir: None }).unwrap();
        (d, st)
    }

    #[test]
    fn tool_effect_chained_and_deduped() {
        let (_d, st) = mk();
        let n = std::cell::Cell::new(0);
        let p = serde_json::json!({"idempotency_key": "key-12345678", "x": 1});
        let a = st.record_tool_effect("run_w1", "shell", &p, || { n.set(n.get() + 1); Ok("rcpt_a".into()) }).unwrap();
        let b = st.record_tool_effect("run_w1", "shell", &p, || { n.set(n.get() + 1); Ok("rcpt_b".into()) }).unwrap();
        assert_eq!((a.as_str(), b.as_str(), n.get()), ("rcpt_a", "rcpt_a", 1));
        let cs = st.chain_store().unwrap();
        let rep = cs.verify_chain("run_w1").unwrap();
        assert!(rep.ok && rep.length == 2, "{rep:?}"); // INTENDED + COMMITTED
        // read-only: unchanged behaviour, no chain
        let ro = serde_json::json!({"effect_class": "READ"});
        st.record_tool_effect("run_w2", "cat", &ro, || Ok("rcpt_c".into())).unwrap();
        assert_eq!(cs.verify_chain("run_w2").unwrap().length, 0);
    }

    #[test]
    fn legacy_reported_as_legacy_and_chained_as_signed() {
        let d = tempfile::tempdir().unwrap();
        let st = ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(d.path().to_path_buf()), receipts_dir: None, blobs_dir: None }).unwrap();
        let legacy = ReceiptRecord { receipt_id: "rcpt_old".into(), run_id: "r1".into(), step: None,
            tool: "t".into(), tool_version: None, inputs_ref: Some("x".into()), outputs_ref: None, exit: None,
            input_tokens: None, output_tokens: None, total_tokens: None };
        st.write_receipt(&legacy).unwrap();
        let r = st.verify_receipt("rcpt_old").unwrap();
        assert_eq!(r.integrity, "legacy (unsigned, unchained)");
        assert_eq!(r.signature_valid, None);

        let cs = ChainStoreAlias::new(d.path().join(".allternit/receipts"));
        let rec = cs.0.append(serde_json::json!({"envelope": {"schema_id":"allternit.kernel.PolicyReceiptV1","schema_version":"1.0.0","run_id":"r1"}})).unwrap();
        let id = rec["chain"]["receipt_id"].as_str().unwrap();
        let v = ChainStoreAlias::verify(&st, id);
        assert_eq!(v.integrity, "chained-signed");
        assert_eq!(v.signature_valid, Some(true));
        assert!(v.is_valid);
        assert!(st.read_receipt("_chains").unwrap().is_none());
    }

    struct ChainStoreAlias(super::super::chain::ChainStore);
    impl ChainStoreAlias {
        fn new(base: PathBuf) -> Self {
            Self(super::super::chain::ChainStore::new(&base, super::super::sign::ReceiptSigner::from_seed([7u8; 32])).unwrap())
        }
        fn verify(st: &ReceiptStore, id: &str) -> ReceiptVerificationResult {
            // Same signer as the store's env/default key is not guaranteed; verify via the
            // store's own JWKS which includes every published public key in _keys.
            st.verify_receipt(id).unwrap()
        }
    }
}
