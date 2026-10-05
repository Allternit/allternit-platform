//! Tamper-evident per-run receipt chain (Decision Q14, ABI 1.0.0 `ReceiptChainV1`).
//!
//! * `chain.content_hash` = `sha256:` of RFC 8785 JCS of the receipt minus
//!   `chain.content_hash` and `chain.signature` (so `index`, `prev_hash`,
//!   `run_id` and the whole body are bound).
//! * Signed material = JCS `{schema_id, schema_version, domain, content_hash}`,
//!   Ed25519, domain `allternit.receipt.v1`.
//! * Frozen schema says `prev_hash` is `null` at index 0 (the task brief
//!   suggested a run_id-derived genesis constant; the frozen schema wins, and
//!   run_id is still bound through the hashed body).
//! * Effect (ActionReceiptV1) receipts double as idempotency records (Q10).

use super::jcs::{canonicalize, hash_value, sha256_tagged};
use super::sign::{jwk_for, verify_sig, Jwks, ReceiptSigner, DOMAIN_TAG};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static PROCESS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChainBreak {
    pub index: u64,
    pub receipt_id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainReport {
    pub run_id: String,
    pub length: u64,
    pub ok: bool,
    pub first_break: Option<ChainBreak>,
}

pub struct ChainStore {
    chains_dir: PathBuf,
    effects_dir: PathBuf,
    keys_dir: PathBuf,
    signer: ReceiptSigner,
}

fn safe_component(s: &str) -> Result<&str> {
    if s.is_empty() || s.contains(['/', '\\']) || s.starts_with('.') || s.contains('\0') {
        bail!("invalid id component: {s:?}");
    }
    Ok(s)
}

/// Content hash of a receipt body (chain.content_hash/signature excluded).
pub fn content_hash(receipt: &Value) -> Result<String> {
    let mut v = receipt.clone();
    if let Some(c) = v.get_mut("chain").and_then(|c| c.as_object_mut()) {
        c.remove("content_hash");
        c.remove("signature");
    }
    hash_value(&v)
}

fn signed_material(schema_id: &str, schema_version: &str, hash: &str) -> Result<Vec<u8>> {
    Ok(canonicalize(&json!({
        "schema_id": schema_id, "schema_version": schema_version,
        "domain": DOMAIN_TAG, "content_hash": hash
    }))?
    .into_bytes())
}

fn s<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut c = v;
    for p in path {
        c = c.get(*p)?;
    }
    c.as_str()
}

impl ChainStore {
    /// `base` is the receipts directory; state goes in `_chains`, `_effects`, `_keys`.
    pub fn new(base: &Path, signer: ReceiptSigner) -> Result<Self> {
        let me = Self {
            chains_dir: base.join("_chains"),
            effects_dir: base.join("_effects"),
            keys_dir: base.join("_keys"),
            signer,
        };
        for d in [&me.chains_dir, &me.effects_dir, &me.keys_dir] {
            std::fs::create_dir_all(d)?;
        }
        // Publish our public key so verifiers/JWKS keep working after rotation.
        let pubfile = me.keys_dir.join(format!("{}.jwk.json", me.signer.kid()));
        if !pubfile.exists() {
            std::fs::write(&pubfile, serde_json::to_vec(&me.signer.jwk())?)?;
        }
        Ok(me)
    }

    /// Default signer: env `ALLTERNIT_RECEIPT_SIGNING_KEY` path or `<base>/_keys/receipt-signing.key`.
    pub fn open(base: &Path) -> Result<Self> {
        let signer = ReceiptSigner::from_env_or_default(&base.join("_keys/receipt-signing.key"))?;
        Self::new(base, signer)
    }

    /// All public keys known to this store, as JWKS (safe to publish).
    pub fn jwks(&self) -> Result<Jwks> {
        let mut keys = vec![self.signer.jwk()];
        for e in std::fs::read_dir(&self.keys_dir)?.flatten() {
            if e.file_name().to_string_lossy().ends_with(".jwk.json") {
                if let Ok(k) = serde_json::from_slice::<super::sign::Jwk>(&std::fs::read(e.path())?) {
                    if !keys.iter().any(|x: &super::sign::Jwk| x.kid == k.kid) {
                        keys.push(k);
                    }
                }
            }
        }
        keys.sort_by(|a, b| a.kid.cmp(&b.kid));
        Ok(Jwks { keys })
    }

    fn run_dir(&self, run_id: &str) -> Result<PathBuf> {
        Ok(self.chains_dir.join(safe_component(run_id)?))
    }

    fn list(&self, run_id: &str) -> Result<Vec<(u64, PathBuf)>> {
        let dir = self.run_dir(run_id)?;
        let mut v = vec![];
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if let Some(i) = n.strip_suffix(".json").and_then(|x| x.parse::<u64>().ok()) {
                    v.push((i, e.path()));
                }
            }
        }
        v.sort();
        Ok(v)
    }

    /// Atomically append `body` to its run's chain, filling `chain.*` and signing.
    /// `body` needs `envelope.{schema_id,schema_version,run_id}`; `chain` may carry
    /// `receipt_id`, `supersedes`, `idempotency_key`.
    pub fn append(&self, mut body: Value) -> Result<Value> {
        let run_id = s(&body, &["envelope", "run_id"]).ok_or_else(|| anyhow!("missing envelope.run_id"))?.to_string();
        let schema_id = s(&body, &["envelope", "schema_id"]).ok_or_else(|| anyhow!("missing envelope.schema_id"))?.to_string();
        let schema_version = s(&body, &["envelope", "schema_version"]).ok_or_else(|| anyhow!("missing envelope.schema_version"))?.to_string();
        let dir = self.run_dir(&run_id)?;
        std::fs::create_dir_all(&dir)?;

        let _g = PROCESS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _f = FileLock::acquire(&dir.join(".lock"))?;

        let existing = self.list(&run_id)?;
        let (index, prev_hash) = match existing.last() {
            None => (0u64, Value::Null),
            Some((i, p)) => {
                let last: Value = serde_json::from_slice(&std::fs::read(p)?)?;
                (i + 1, last["chain"]["content_hash"].clone())
            }
        };
        let old = body.get("chain").and_then(|c| c.as_object()).cloned().unwrap_or_default();
        let receipt_id = old.get("receipt_id").and_then(|x| x.as_str()).map(String::from)
            .unwrap_or_else(|| format!("rcpt_{}", uuid::Uuid::new_v4().simple()));
        let mut chain = Map::new();
        chain.insert("receipt_id".into(), json!(receipt_id));
        chain.insert("run_id".into(), json!(run_id));
        chain.insert("index".into(), json!(index));
        chain.insert("prev_hash".into(), prev_hash);
        chain.insert("schema_id".into(), json!(schema_id));
        chain.insert("schema_version".into(), json!(schema_version));
        chain.insert("domain".into(), json!(DOMAIN_TAG));
        chain.insert("canonicalization".into(), json!("RFC8785-JCS"));
        for k in ["supersedes", "idempotency_key", "extensions"] {
            if let Some(x) = old.get(k) {
                chain.insert(k.into(), x.clone());
            }
        }
        body["chain"] = Value::Object(chain);
        let hash = content_hash(&body)?;
        let sig = self.signer.sign(&signed_material(&schema_id, &schema_version, &hash)?);
        body["chain"]["content_hash"] = json!(hash);
        body["chain"]["signature"] = json!({
            "alg": "ed25519", "key_id": self.signer.kid(), "value": sig, "domain": DOMAIN_TAG
        });

        let path = dir.join(format!("{index:010}.json"));
        if path.exists() {
            bail!("chain slot {index} already exists");
        }
        let tmp = dir.join(format!(".{index:010}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&body)?)?;
        std::fs::rename(&tmp, &path)?;
        // Head pointer: detects tail truncation.
        std::fs::write(dir.join("head.json"), serde_json::to_vec(&json!({"index": index, "content_hash": hash}))?)?;
        Ok(body)
    }

    pub fn read_run(&self, run_id: &str) -> Result<Vec<Value>> {
        self.list(run_id)?.into_iter()
            .map(|(_, p)| Ok(serde_json::from_slice(&std::fs::read(&p)?)?))
            .collect()
    }

    /// Find a chained receipt by id across all runs.
    pub fn find_by_id(&self, receipt_id: &str) -> Result<Option<Value>> {
        for e in std::fs::read_dir(&self.chains_dir)?.flatten() {
            if !e.path().is_dir() { continue; }
            for (_, p) in self.list(&e.file_name().to_string_lossy())? {
                let v: Value = serde_json::from_slice(&std::fs::read(&p)?)?;
                if s(&v, &["chain", "receipt_id"]) == Some(receipt_id) {
                    return Ok(Some(v));
                }
            }
        }
        Ok(None)
    }

    pub fn verify_chain(&self, run_id: &str) -> Result<ChainReport> {
        self.verify_chain_with(run_id, &self.jwks()?)
    }

    /// Verify against an explicit key set (e.g. a published JWKS).
    pub fn verify_chain_with(&self, run_id: &str, jwks: &Jwks) -> Result<ChainReport> {
        let entries = self.list(run_id)?;
        let mut prev: Option<String> = None;
        let brk = |i: u64, rid: Option<String>, r: &str| ChainReport {
            run_id: run_id.into(), length: entries.len() as u64, ok: false,
            first_break: Some(ChainBreak { index: i, receipt_id: rid, reason: r.into() }),
        };
        for (pos, (idx, path)) in entries.iter().enumerate() {
            let pos = pos as u64;
            let v: Value = match std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
                Some(v) => v,
                None => return Ok(brk(pos, None, "receipt file unreadable")),
            };
            let rid = s(&v, &["chain", "receipt_id"]).map(String::from);
            if *idx != pos { return Ok(brk(pos, rid, "missing receipt (index gap)")); }
            if v["chain"]["index"].as_u64() != Some(pos) { return Ok(brk(pos, rid, "chain.index mismatch")); }
            if s(&v, &["chain", "run_id"]) != Some(run_id) || s(&v, &["envelope", "run_id"]) != Some(run_id) {
                return Ok(brk(pos, rid, "run_id mismatch"));
            }
            let want_prev = prev.as_deref().map(Value::from).unwrap_or(Value::Null);
            if v["chain"]["prev_hash"] != want_prev { return Ok(brk(pos, rid, "prev_hash does not match previous receipt")); }
            let hash = match content_hash(&v) { Ok(h) => h, Err(_) => return Ok(brk(pos, rid, "body not canonicalizable")) };
            if s(&v, &["chain", "content_hash"]) != Some(hash.as_str()) { return Ok(brk(pos, rid, "content_hash mismatch (body tampered)")); }
            let (sid, sver) = (s(&v, &["chain", "schema_id"]).unwrap_or(""), s(&v, &["chain", "schema_version"]).unwrap_or(""));
            if s(&v, &["envelope", "schema_id"]) != Some(sid) || s(&v, &["envelope", "schema_version"]) != Some(sver) {
                return Ok(brk(pos, rid, "schema id/version mismatch"));
            }
            let sig = &v["chain"]["signature"];
            if !sig.is_object() { return Ok(brk(pos, rid, "missing signature")); }
            let Some(vk) = s(sig, &["key_id"]).and_then(|k| jwks.find(k)) else {
                return Ok(brk(pos, rid, "unknown signing key id"));
            };
            let msg = signed_material(sid, sver, &hash)?;
            if s(sig, &["domain"]) != Some(DOMAIN_TAG) || !verify_sig(&vk, &msg, s(sig, &["value"]).unwrap_or("")) {
                return Ok(brk(pos, rid, "signature invalid"));
            }
            prev = Some(hash);
        }
        // Tail truncation check against the head pointer.
        if let Ok(h) = std::fs::read(self.run_dir(run_id)?.join("head.json")) {
            if let Ok(h) = serde_json::from_slice::<Value>(&h) {
                let want = h["index"].as_u64().map(|i| i + 1).unwrap_or(0);
                if want != entries.len() as u64 {
                    return Ok(brk(entries.len() as u64, None, "chain truncated (head pointer ahead of tail)"));
                }
            }
        }
        Ok(ChainReport { run_id: run_id.into(), length: entries.len() as u64, ok: true, first_break: None })
    }
}

/// Verdict for one chained receipt: its own hash/signature plus every link up to it.
pub fn verify_chained(cs: &ChainStore, receipt_id: &str, v: &Value) -> super::store::ReceiptVerificationResult {
    let run = s(v, &["chain", "run_id"]).unwrap_or("").to_string();
    let idx = v["chain"]["index"].as_u64().unwrap_or(0);
    let mut errors = vec![];
    let (mut hash_ok, mut sig_ok) = (true, true);
    match cs.verify_chain(&run) {
        Ok(rep) => {
            if let Some(b) = rep.first_break.filter(|b| b.index <= idx || b.receipt_id.is_none()) {
                if b.reason.contains("signature") || b.reason.contains("key") || b.reason.contains("missing signature") { sig_ok = false; } else { hash_ok = false; }
                errors.push(format!("chain broken at index {}: {}", b.index, b.reason));
            }
        }
        Err(e) => { hash_ok = false; errors.push(format!("chain verification error: {e}")); }
    }
    super::store::ReceiptVerificationResult {
        receipt_id: receipt_id.into(), is_valid: errors.is_empty(), hash_matches: hash_ok,
        signature_valid: Some(sig_ok && errors.is_empty()), integrity: "chained-signed".into(), errors,
    }
}

struct FileLock(#[allow(dead_code)] std::fs::File);
impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        let f = std::fs::OpenOptions::new().create(true).write(true).open(path).context("open chain lock")?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
                bail!("flock failed: {}", std::io::Error::last_os_error());
            }
        }
        Ok(Self(f)) // lock released when the file closes
    }
}

// ---------------------------------------------------------------- effects

#[derive(Debug, Clone)]
pub struct EffectContext {
    pub run_id: String,
    pub session_id: String,
    pub task_id: String,
    pub node_id: Option<String>,
    pub trace_id: String,
    pub state_version: u64,
    pub producer_id: String,
    pub policy_decision_id: String,
}

#[derive(Debug, Clone)]
pub struct EffectRequest {
    pub action_id: String,
    pub tool_id: String,
    pub args_hash: String,
    pub idempotency_key: String,
    pub effect_class: String,
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EffectOutcome {
    /// Executed now; the COMMITTED/FAILED receipt.
    Executed(Value),
    /// A COMMITTED receipt already existed for this key; nothing was re-executed.
    Replayed(Value),
}

impl ChainStore {
    fn effect_index(&self, key: &str) -> PathBuf {
        self.effects_dir.join(format!("{}.json", hex::encode(sha2::Sha256::digest(key.as_bytes()))))
    }

    /// Record one ActionReceiptV1 (chained, signed, durable) and index it by idempotency key.
    pub fn record_effect(&self, cx: &EffectContext, r: &EffectRequest, status: &str,
                         result_hash: Option<&str>, external_ref: Option<&str>, supersedes: Option<&str>) -> Result<Value> {
        if r.idempotency_key.len() < 8 { bail!("idempotency_key must be >= 8 chars"); }
        if !r.args_hash.starts_with("sha256:") { bail!("args_hash must be sha256:<hex>"); }
        let mut chain = Map::new();
        chain.insert("idempotency_key".into(), json!(r.idempotency_key));
        if let Some(sp) = supersedes { chain.insert("supersedes".into(), json!(sp)); }
        let body = json!({
            "envelope": {
                "abi_version": "1.0.0",
                "schema_id": "allternit.kernel.ActionReceiptV1",
                "schema_version": "1.0.0",
                "run_id": cx.run_id, "session_id": cx.session_id, "task_id": cx.task_id,
                "node_id": cx.node_id,
                "state_version": cx.state_version,
                "created_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "producer": {"component_id": cx.producer_id, "component_version": "1", "kind": "RUNTIME"},
                "trace_id": cx.trace_id,
                "provenance": [{"source_type": "TOOL", "source_id": r.tool_id, "trust_class": "INTERNAL",
                                "content_hash": result_hash}]
            },
            "chain": chain,
            "action_id": r.action_id,
            "effect_class": r.effect_class,
            "idempotency_key": r.idempotency_key,
            "target": r.target,
            "status": status,
            "policy_decision_id": cx.policy_decision_id,
            "external_ref": external_ref,
            "result_hash": result_hash,
            "extensions": {"x-tool-id": r.tool_id, "x-args-hash": r.args_hash}
        });
        let rec = self.append(body)?;
        let idx = json!({"run_id": cx.run_id, "index": rec["chain"]["index"], "receipt_id": rec["chain"]["receipt_id"]});
        let p = self.effect_index(&r.idempotency_key);
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&idx)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(rec)
    }

    /// Latest recorded effect receipt for `idempotency_key`, if any. This is the raw
    /// index lookup; it neither authenticates the receipt nor binds it to an operation.
    /// Callers deciding whether an effect may run use [`ChainStore::run_effect_once`] /
    /// [`ChainStore::reserve_effect`], which do both.
    pub fn find_effect(&self, idempotency_key: &str) -> Result<Option<Value>> {
        let Ok(b) = std::fs::read(self.effect_index(idempotency_key)) else { return Ok(None) };
        let idx: Value = serde_json::from_slice(&b)?;
        let (run, i) = (idx["run_id"].as_str().unwrap_or(""), idx["index"].as_u64().unwrap_or(u64::MAX));
        let p = self.run_dir(run)?.join(format!("{i:010}.json"));
        Ok(std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()))
    }

    /// Per-key lock spanning lookup, INTENDED publication, execution and the terminal
    /// receipt (review #9). Cross-process (flock) and cross-thread (a separate open
    /// file description per acquire).
    fn key_lock(&self, key: &str) -> Result<FileLock> {
        FileLock::acquire(&self.effect_index(key).with_extension("lock"))
    }

    /// Prior effect for `r`'s key, authenticated (signature + chain up to it) and
    /// bound to the same operation (review #15). A key reused for a different
    /// run/tool/arguments/effect class/target is a conflict, never a replay.
    fn prior_effect(&self, cx: &EffectContext, r: &EffectRequest) -> Result<Option<Value>> {
        let Some(prev) = self.find_effect(&r.idempotency_key)? else { return Ok(None) };
        let rid = s(&prev, &["chain", "receipt_id"]).unwrap_or("").to_string();
        let v = verify_chained(self, &rid, &prev);
        if !v.is_valid {
            bail!("idempotency key {} resolves to an unauthenticated effect receipt {rid}: {:?}", r.idempotency_key, v.errors);
        }
        let prev_op = effect_op_hash(
            s(&prev, &["envelope", "run_id"]).unwrap_or(""),
            s(&prev, &["extensions", "x-tool-id"]).unwrap_or(""),
            s(&prev, &["extensions", "x-args-hash"]).unwrap_or(""),
            prev["effect_class"].as_str().unwrap_or(""),
            prev["target"].as_str(),
        )?;
        if prev_op != effect_op_hash(&cx.run_id, &r.tool_id, &r.args_hash, &r.effect_class, r.target.as_deref())? {
            bail!("idempotency key conflict: {} was already used for a different operation (receipt {rid})", r.idempotency_key);
        }
        Ok(Some(prev))
    }

    /// Execute `exec` at most once per idempotency key. Writes INTENDED before the
    /// effect and COMMITTED/FAILED after. A retry after COMMITTED returns the recorded
    /// receipt; a retry after a bare INTENDED (crash mid-effect) is an error requiring
    /// reconciliation, since the effect may or may not have happened. The whole
    /// sequence runs under a per-key lock: a concurrent duplicate waits and then
    /// gets `Replayed`; it never re-executes (review #9).
    pub fn run_effect_once<F>(&self, cx: &EffectContext, r: &EffectRequest, exec: F) -> Result<EffectOutcome>
    where F: FnOnce() -> Result<(String, Option<String>)> {
        self.effect_locked(cx, r, exec, false)
    }

    /// Settle an effect reserved earlier with [`ChainStore::reserve_effect`]: a
    /// matching INTENDED is superseded by COMMITTED/FAILED. With no reservation this
    /// behaves like [`ChainStore::run_effect_once`].
    pub fn complete_effect<F>(&self, cx: &EffectContext, r: &EffectRequest, exec: F) -> Result<EffectOutcome>
    where F: FnOnce() -> Result<(String, Option<String>)> {
        self.effect_locked(cx, r, exec, true)
    }

    fn effect_locked<F>(&self, cx: &EffectContext, r: &EffectRequest, exec: F, settle_intended: bool) -> Result<EffectOutcome>
    where F: FnOnce() -> Result<(String, Option<String>)> {
        if r.idempotency_key.len() < 8 { bail!("idempotency_key must be >= 8 chars"); }
        let _k = self.key_lock(&r.idempotency_key)?;
        let iid = match self.prior_effect(cx, r)? {
            Some(prev) => match prev["status"].as_str() {
                Some("COMMITTED") => return Ok(EffectOutcome::Replayed(prev)),
                Some("INTENDED") if settle_intended => s(&prev, &["chain", "receipt_id"]).unwrap_or("").to_string(),
                Some("INTENDED") | Some("UNKNOWN") => bail!(
                    "effect {} has an unresolved INTENDED receipt; reconcile before retrying", r.idempotency_key),
                _ => String::new(), // FAILED / COMPENSATED: a fresh attempt is allowed
            },
            None => String::new(),
        };
        let iid = if iid.is_empty() {
            let intent = self.record_effect(cx, r, "INTENDED", None, None, None)?;
            intent["chain"]["receipt_id"].as_str().unwrap_or("").to_string()
        } else { iid };
        match exec() {
            Ok((result_hash, ext)) => Ok(EffectOutcome::Executed(
                self.record_effect(cx, r, "COMMITTED", Some(&result_hash), ext.as_deref(), Some(&iid))?)),
            Err(_) => Ok(EffectOutcome::Executed(self.record_effect(cx, r, "FAILED", None, None, Some(&iid))?)),
        }
    }

    /// Pre-effect admission (review #10): atomically claim `r`'s key BEFORE the tool
    /// runs (reserve -> execute -> [`ChainStore::complete_effect`]). A key already
    /// COMMITTED for the same operation returns that receipt (do not execute); a key
    /// already reserved or unresolved is refused (another caller holds it).
    pub fn reserve_effect(&self, cx: &EffectContext, r: &EffectRequest) -> Result<EffectReservation> {
        if r.idempotency_key.len() < 8 { bail!("idempotency_key must be >= 8 chars"); }
        let _k = self.key_lock(&r.idempotency_key)?;
        if let Some(prev) = self.prior_effect(cx, r)? {
            match prev["status"].as_str() {
                Some("COMMITTED") => return Ok(EffectReservation::Committed(prev)),
                Some("INTENDED") | Some("UNKNOWN") => bail!(
                    "effect {} is already reserved or unresolved; not executed", r.idempotency_key),
                _ => {}
            }
        }
        Ok(EffectReservation::Reserved(self.record_effect(cx, r, "INTENDED", None, None, None)?))
    }
}

/// Outcome of [`ChainStore::reserve_effect`].
#[derive(Debug, Clone)]
pub enum EffectReservation {
    /// The INTENDED receipt: the caller now owns the key and may execute once.
    Reserved(Value),
    /// Already COMMITTED for this operation: do not execute; this is the result.
    Committed(Value),
}

/// Operation identity an idempotency key is bound to: run + tool + canonical args
/// hash + effect class + target.
pub fn effect_op_hash(run_id: &str, tool_id: &str, args_hash: &str, effect_class: &str, target: Option<&str>) -> Result<String> {
    hash_value(&json!({"run_id": run_id, "tool_id": tool_id, "args_hash": args_hash,
                       "effect_class": effect_class, "target": target}))
}

use sha2::Digest;
#[allow(dead_code)]
fn _unused() { let _ = (sha256_tagged, jwk_for); }

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ChainStore) {
        let d = tempfile::tempdir().unwrap();
        let s = ChainStore::new(d.path(), ReceiptSigner::generate()).unwrap();
        (d, s)
    }
    fn body(run: &str, n: u32) -> Value {
        json!({"envelope": {"abi_version":"1.0.0","schema_id":"allternit.kernel.PolicyReceiptV1","schema_version":"1.0.0","run_id":run},
               "n": n, "decision": "ALLOW"})
    }
    fn fill(s: &ChainStore, run: &str, k: u32) { for n in 0..k { s.append(body(run, n)).unwrap(); } }
    fn rd(d: &tempfile::TempDir, run: &str, i: u64) -> PathBuf { d.path().join(format!("_chains/{run}/{i:010}.json")) }

    #[test]
    fn chain_verifies_and_links() {
        let (_d, s) = store();
        fill(&s, "run1", 4);
        let r = s.read_run("run1").unwrap();
        assert!(r[0]["chain"]["prev_hash"].is_null());
        assert_eq!(r[1]["chain"]["prev_hash"], r[0]["chain"]["content_hash"]);
        let rep = s.verify_chain("run1").unwrap();
        assert!(rep.ok && rep.length == 4, "{rep:?}");
        assert!(s.verify_chain("empty").unwrap().ok);
    }

    #[test]
    fn tampered_body_detected() {
        let (d, s) = store();
        fill(&s, "run1", 3);
        let p = rd(&d, "run1", 1);
        let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        v["decision"] = json!("DENY");
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        let b = s.verify_chain("run1").unwrap().first_break.unwrap();
        assert_eq!(b.index, 1);
        assert!(b.reason.contains("tampered"));
    }

    #[test]
    fn removed_and_reordered_detected() {
        let (d, s) = store();
        fill(&s, "a", 4);
        std::fs::remove_file(rd(&d, "a", 1)).unwrap();
        assert_eq!(s.verify_chain("a").unwrap().first_break.unwrap().index, 1);
        // truncated tail
        fill(&s, "b", 3);
        std::fs::remove_file(rd(&d, "b", 2)).unwrap();
        assert!(s.verify_chain("b").unwrap().first_break.unwrap().reason.contains("truncated"));
        // reorder: swap two files' contents
        fill(&s, "c", 3);
        let (p1, p2) = (rd(&d, "c", 1), rd(&d, "c", 2));
        let (x, y) = (std::fs::read(&p1).unwrap(), std::fs::read(&p2).unwrap());
        std::fs::write(&p1, y).unwrap();
        std::fs::write(&p2, x).unwrap();
        assert_eq!(s.verify_chain("c").unwrap().first_break.unwrap().index, 1);
    }

    #[test]
    fn wrong_key_detected() {
        let (_d, s) = store();
        fill(&s, "run1", 2);
        let other = Jwks { keys: vec![ReceiptSigner::generate().jwk()] };
        let b = s.verify_chain_with("run1", &other).unwrap().first_break.unwrap();
        assert_eq!(b.index, 0);
        // Forged re-sign with another key but original kid claim:
        let forger = ReceiptSigner::generate();
        let mut k = forger.jwk();
        k.kid = s.signer.kid().to_string();
        let forged = Jwks { keys: vec![k] };
        assert!(s.verify_chain_with("run1", &forged).unwrap().first_break.unwrap().reason.contains("signature"));
    }

    #[test]
    fn jwks_has_no_private_material() {
        let (_d, s) = store();
        let j = serde_json::to_string(&s.jwks().unwrap()).unwrap();
        assert!(j.contains("Ed25519") && !j.contains("\"d\""));
    }

    fn cx() -> EffectContext {
        EffectContext { run_id: "runE".into(), session_id: "s1".into(), task_id: "t1".into(), node_id: None,
            trace_id: "tr1".into(), state_version: 1, producer_id: "commrails".into(), policy_decision_id: "dec1".into() }
    }
    fn req(key: &str) -> EffectRequest {
        EffectRequest { action_id: "act1".into(), tool_id: "fs.write".into(),
            args_hash: sha256_tagged(b"args"), idempotency_key: key.into(),
            effect_class: "WORKSPACE_WRITE".into(), target: Some("a.txt".into()) }
    }

    #[test]
    fn idempotent_effect_lookup_and_replay() {
        let (_d, s) = store();
        let mut calls = 0;
        let o1 = s.run_effect_once(&cx(), &req("idem-key-1"), || { calls += 1; Ok((sha256_tagged(b"res"), Some("ext1".into()))) }).unwrap();
        assert!(matches!(o1, EffectOutcome::Executed(_)));
        let o2 = s.run_effect_once(&cx(), &req("idem-key-1"), || { calls += 1; Ok((sha256_tagged(b"other"), None)) }).unwrap();
        let EffectOutcome::Replayed(r) = o2 else { panic!("expected replay") };
        assert_eq!(calls, 1);
        assert_eq!(r["result_hash"], json!(sha256_tagged(b"res")));
        assert_eq!(s.find_effect("idem-key-1").unwrap().unwrap()["status"], "COMMITTED");
        assert!(s.find_effect("nope-nope-1").unwrap().is_none());
        assert!(s.verify_chain("runE").unwrap().ok);
        // INTENDED-only (crash) blocks a blind retry
        s.record_effect(&cx(), &req("idem-key-2"), "INTENDED", None, None, None).unwrap();
        assert!(s.run_effect_once(&cx(), &req("idem-key-2"), || Ok((sha256_tagged(b"x"), None))).is_err());
        assert!(s.record_effect(&cx(), &req("short"), "INTENDED", None, None, None).is_err());
    }

    #[test]
    fn review9_concurrent_identical_requests_execute_once() {
        let (_d, s) = store();
        let s = std::sync::Arc::new(s);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let bar = std::sync::Arc::new(std::sync::Barrier::new(2));
        let hs: Vec<_> = (0..2).map(|_| {
            let (s, calls, bar) = (s.clone(), calls.clone(), bar.clone());
            std::thread::spawn(move || {
                bar.wait();
                s.run_effect_once(&cx(), &req("pay-key-0001"), || {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    Ok((sha256_tagged(b"paid"), Some("pay-ref".into())))
                }).unwrap()
            })
        }).collect();
        let outs: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "effect ran more than once");
        assert_eq!(outs.iter().filter(|o| matches!(o, EffectOutcome::Replayed(_))).count(), 1);
        assert!(s.verify_chain("runE").unwrap().ok);
    }

    #[test]
    fn review10_reserve_before_effect_blocks_duplicate_admission() {
        let (_d, s) = store();
        let EffectReservation::Reserved(_) = s.reserve_effect(&cx(), &req("res-key-0001")).unwrap() else { panic!() };
        // A second pre-tool admission for the same key is refused before any effect.
        assert!(s.reserve_effect(&cx(), &req("res-key-0001")).is_err());
        let o = s.complete_effect(&cx(), &req("res-key-0001"), || Ok((sha256_tagged(b"r"), Some("ext".into())))).unwrap();
        let EffectOutcome::Executed(rec) = o else { panic!() };
        assert_eq!(rec["status"], "COMMITTED");
        let EffectReservation::Committed(c) = s.reserve_effect(&cx(), &req("res-key-0001")).unwrap() else { panic!("must not re-admit") };
        assert_eq!(c["external_ref"], "ext");
        assert!(s.verify_chain("runE").unwrap().ok);
    }

    #[test]
    fn review15_reused_key_for_different_operation_is_conflict() {
        let (_d, s) = store();
        s.run_effect_once(&cx(), &req("shared-key"), || Ok((sha256_tagged(b"a"), Some("extA".into())))).unwrap();
        let mut other = req("shared-key");
        other.tool_id = "tool.publish".into();
        other.args_hash = sha256_tagged(b"argsB");
        let mut ran = false;
        let e = s.run_effect_once(&cx(), &other, || { ran = true; Ok((sha256_tagged(b"b"), None)) }).unwrap_err();
        assert!(e.to_string().contains("conflict"), "{e}");
        assert!(!ran);
        // Same op in another run is also a conflict.
        let mut cx2 = cx();
        cx2.run_id = "runOther".into();
        assert!(s.run_effect_once(&cx2, &req("shared-key"), || Ok((sha256_tagged(b"c"), None))).is_err());
        // A FAILED receipt edited to COMMITTED is not trusted.
        s.run_effect_once(&cx(), &req("fail-key-01"), || anyhow::bail!("boom")).unwrap();
        let prev = s.find_effect("fail-key-01").unwrap().unwrap();
        let p = s.run_dir("runE").unwrap().join(format!("{:010}.json", prev["chain"]["index"].as_u64().unwrap()));
        let mut v = prev.clone();
        v["status"] = json!("COMMITTED");
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        let e = s.run_effect_once(&cx(), &req("fail-key-01"), || Ok((sha256_tagged(b"x"), None))).unwrap_err();
        assert!(e.to_string().contains("unauthenticated"), "{e}");
    }
}
