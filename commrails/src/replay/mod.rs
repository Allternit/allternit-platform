//! WP5 Replay / Cassette (ABI 1.0.0 `CassetteV1`, `DivergenceReportV1`; CL-246).
//!
//! * **Record**: [`record_cassette`] turns a run's verified WP3 receipt chain into a
//!   cassette. Every terminal effect receipt (`ActionReceiptV1` COMMITTED/FAILED)
//!   becomes an effectful `TOOL` entry; policy/spawn receipts become non-effectful
//!   `POLICY`/`DECISION` boundary entries. Bare `INTENDED` effects (unknown outcome)
//!   refuse to record: there is no result to replay.
//! * **Replay**: [`Replayer`] runs with `effects: recorded_only` only. It takes no
//!   executor at all, so nothing can run live: a side-effecting call is answered with
//!   the recorded, content-addressed result, and a call with no recording is refused
//!   and reported as `EXTRA_ENTRY`.
//! * **Divergence**: [`Replayer::finish`] emits a `DivergenceReportV1` (what differed,
//!   at which `seq`). Tampered chains, tampered cassettes, deleted receipts, reordered
//!   and unconsumed entries are all divergences.
//!
//! The serde types below mirror `spec/Contracts/kernel/v1/schemas/replay.schema.json`
//! exactly (closed objects); the tests round-trip the ABI conformance examples.

use crate::receipts::chain::{content_hash, ChainStore};
use crate::receipts::jcs::hash_value;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const ABI_VERSION: &str = "1.0.0";
pub const CASSETTE_SCHEMA_ID: &str = "allternit.kernel.CassetteV1";
pub const REPORT_SCHEMA_ID: &str = "allternit.kernel.DivergenceReportV1";

// ------------------------------------------------------------------ ABI types

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Boundary {
    Decision,
    Tool,
    Capability,
    Policy,
    Verification,
    Mutation,
    Wake,
    Attention,
}

impl Boundary {
    fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_string())).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CassetteEntry {
    pub seq: u64,
    pub node_id: String,
    pub boundary: Boundary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primitive_id: Option<String>,
    pub request_hash: String,
    pub recorded_result_hash: String,
    pub result_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effectful: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_taken: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_ids: Option<Vec<String>>,
}

impl CassetteEntry {
    pub fn is_effectful(&self) -> bool {
        self.effectful.unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CassetteV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub cassette_id: String,
    pub run_id: String,
    pub graph_id: String,
    pub graph_version: u64,
    pub abi_version: String,
    pub entries: Vec<CassetteEntry>,
    pub run_receipt_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Identical,
    ExpectedDivergence,
    UnexpectedDivergence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DivergenceKind {
    Branch,
    ResultHash,
    NodeOrder,
    MissingEntry,
    ExtraEntry,
    PolicyOutcome,
    BackendResolution,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Divergence {
    pub seq: u64,
    pub node_id: String,
    pub kind: DivergenceKind,
    pub expected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DivergenceReportV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub report_id: String,
    pub cassette_id: String,
    pub replay_run_id: String,
    pub verdict: Verdict,
    pub divergences: Vec<Divergence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

/// Replay effect mode. Only `recorded_only` exists: live effects during replay are
/// not a mode this kernel offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EffectsMode {
    #[default]
    RecordedOnly,
}

// ------------------------------------------------------------------ hashing

fn short(h: &str) -> &str {
    h.strip_prefix("sha256:").map(|x| &x[..24.min(x.len())]).unwrap_or(h)
}

fn valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 200 && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._:@-".contains(c))
}

fn id_or(s: Option<&str>, fallback: &str) -> String {
    match s {
        Some(x) if valid_id(x) => x.to_string(),
        _ => fallback.to_string(),
    }
}

/// ABI `PrimitiveId` (`tool.fs_write`, ...). Tool ids that are not canonical
/// primitive ids are left out rather than invented.
fn primitive_id(s: &str) -> Option<String> {
    const P: [&str; 13] = ["obs", "dec", "ctx", "plan", "tool", "mut", "ver", "ctl", "route", "state", "policy", "bg", "mem"];
    let mut parts = s.split('.');
    let head = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let ok = P.contains(&head) && !rest.is_empty()
        && rest.iter().all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'));
    ok.then(|| s.to_string())
}

/// Request hash of a tool boundary: what was asked, independent of idempotency key.
pub fn tool_request_hash(tool_id: &str, args_hash: &str, effect_class: &str) -> Result<String> {
    hash_value(&json!({"boundary": "TOOL", "tool_id": tool_id, "args_hash": args_hash, "effect_class": effect_class}))
}

fn boundary_request_hash(boundary: Boundary, node_id: &str, schema_id: &str) -> Result<String> {
    hash_value(&json!({"boundary": boundary, "node_id": node_id, "schema_id": schema_id}))
}

/// Recorded result hash of an effect receipt: its `result_hash` when COMMITTED,
/// otherwise a hash of the terminal status (a recorded failure is still a result).
fn effect_result_hash(r: &Value) -> Result<String> {
    match (r["status"].as_str(), r["result_hash"].as_str()) {
        (Some("COMMITTED"), Some(h)) => Ok(h.to_string()),
        (st, _) => hash_value(&json!({"status": st})),
    }
}

/// Result hash of a non-effect receipt: the body minus chain bookkeeping and envelope.
fn body_result_hash(r: &Value) -> Result<String> {
    let mut v = r.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("chain");
        o.remove("envelope");
    }
    hash_value(&v)
}

// ------------------------------------------------------------------ record

fn schema_id(r: &Value) -> &str {
    r["envelope"]["schema_id"].as_str().unwrap_or("")
}

fn receipt_id(r: &Value) -> String {
    r["chain"]["receipt_id"].as_str().unwrap_or("").to_string()
}

/// Build a cassette from a run's WP3 receipt chain. The chain must verify.
pub fn record_cassette(cs: &ChainStore, run_id: &str, graph_id: Option<&str>, graph_version: u64) -> Result<CassetteV1> {
    let rep = cs.verify_chain(run_id)?;
    if !rep.ok {
        bail!("receipt chain for run {run_id} does not verify: {:?}", rep.first_break);
    }
    let receipts = cs.read_run(run_id)?;
    let head = receipts.last().ok_or_else(|| anyhow!("run {run_id} has no receipts to record"))?;
    let run_receipt_hash = head["chain"]["content_hash"].as_str().unwrap_or("").to_string();
    let run_node = id_or(Some(run_id), "run");

    // INTENDED receipts that a later terminal receipt superseded.
    let superseded: HashMap<String, String> = receipts.iter()
        .filter_map(|r| Some((r["chain"]["supersedes"].as_str()?.to_string(), receipt_id(r))))
        .collect();

    let mut entries = Vec::new();
    for r in &receipts {
        let sid = schema_id(r);
        let rid = receipt_id(r);
        let node_id = id_or(r["envelope"]["node_id"].as_str(), &run_node);
        let x_boundary = r["extensions"]["x-boundary"].as_str().and_then(Boundary::parse);
        let seq = entries.len() as u64;
        match sid {
            "allternit.kernel.ActionReceiptV1" => {
                match r["status"].as_str() {
                    Some("INTENDED") | Some("UNKNOWN") => {
                        if superseded.contains_key(&rid) { continue; }
                        bail!("run {run_id} has an unresolved {} effect receipt {rid}; reconcile before recording",
                              r["status"].as_str().unwrap_or(""));
                    }
                    _ => {}
                }
                let tool_id = r["extensions"]["x-tool-id"].as_str().unwrap_or("");
                let args_hash = r["extensions"]["x-args-hash"].as_str().unwrap_or("");
                let class = r["effect_class"].as_str().unwrap_or("");
                let mut ids = vec![];
                if let Some(sp) = r["chain"]["supersedes"].as_str() { ids.push(sp.to_string()); }
                ids.push(rid.clone());
                entries.push(CassetteEntry {
                    seq, node_id, boundary: x_boundary.unwrap_or(Boundary::Tool),
                    primitive_id: primitive_id(tool_id),
                    request_hash: tool_request_hash(tool_id, args_hash, class)?,
                    recorded_result_hash: effect_result_hash(r)?,
                    result_ref: rid, effectful: Some(true), branch_taken: None, receipt_ids: Some(ids),
                });
            }
            // The run summary is not a boundary.
            "allternit.kernel.RunReceiptV1" => continue,
            _ => {
                let boundary = x_boundary.unwrap_or(if sid == "allternit.kernel.PolicyReceiptV1" {
                    Boundary::Policy
                } else {
                    Boundary::Decision
                });
                let branch = r["decision"].as_str().or_else(|| r["branch"].as_str()).map(String::from);
                entries.push(CassetteEntry {
                    seq, boundary,
                    primitive_id: None,
                    request_hash: boundary_request_hash(boundary, &node_id, sid)?,
                    node_id,
                    recorded_result_hash: body_result_hash(r)?,
                    result_ref: rid.clone(), effectful: Some(false), branch_taken: branch, receipt_ids: Some(vec![rid]),
                });
            }
        }
    }
    if entries.is_empty() {
        bail!("run {run_id} has no replayable boundaries");
    }
    let cassette_id = format!("cas_{}", short(&hash_value(&json!({"run": run_id, "head": run_receipt_hash}))?));
    Ok(CassetteV1 {
        schema_id: CASSETTE_SCHEMA_ID.into(), schema_version: "1.0.0".into(), cassette_id,
        run_id: run_id.into(), graph_id: id_or(graph_id, &run_node), graph_version,
        abi_version: ABI_VERSION.into(), entries, run_receipt_hash,
        created_at: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        extensions: None,
    })
}

/// `<receipts_dir>/_cassettes/<cassette_id>.json`
pub fn save_cassette(dir: &Path, c: &CassetteV1) -> Result<PathBuf> {
    if !valid_id(&c.cassette_id) { bail!("invalid cassette_id"); }
    std::fs::create_dir_all(dir)?;
    let p = dir.join(format!("{}.json", c.cassette_id));
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(c)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

pub fn load_cassette(path: &Path) -> Result<CassetteV1> {
    let c: CassetteV1 = serde_json::from_slice(&std::fs::read(path)?)?;
    if c.schema_id != CASSETTE_SCHEMA_ID { bail!("not a CassetteV1: {}", c.schema_id); }
    Ok(c)
}

// ------------------------------------------------------------------ replay

/// One boundary crossed by the replaying run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStep {
    pub boundary: Boundary,
    pub node_id: String,
    pub request_hash: String,
    /// Non-effect boundaries: the branch the replay took (compared, not served).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Non-effect boundaries: the result the replay produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    /// Effect boundaries: the WP3 idempotency key, cross-checked against the chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

impl ReplayStep {
    /// A side-effecting tool call; `args` is hashed exactly as the gate records it.
    pub fn tool(node_id: &str, tool_id: &str, args: &Value, effect_class: &str) -> Result<Self> {
        Ok(Self {
            boundary: Boundary::Tool, node_id: node_id.into(),
            request_hash: tool_request_hash(tool_id, &hash_value(args)?, effect_class)?,
            branch: None, result_hash: None, idempotency_key: None,
        })
    }
    pub fn with_key(mut self, k: &str) -> Self { self.idempotency_key = Some(k.into()); self }

    /// The steps the recording itself took (self-replay / integrity check).
    pub fn from_cassette(c: &CassetteV1) -> Vec<Self> {
        c.entries.iter().map(|e| Self {
            boundary: e.boundary, node_id: e.node_id.clone(), request_hash: e.request_hash.clone(),
            branch: e.branch_taken.clone(),
            result_hash: (!e.is_effectful()).then(|| e.recorded_result_hash.clone()),
            idempotency_key: None,
        }).collect()
    }
}

/// The recorded answer to a boundary; for effects, what the tool returned live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedResult {
    pub seq: u64,
    pub receipt_id: String,
    pub result_hash: String,
    pub status: Option<String>,
    pub external_ref: Option<String>,
    pub branch_taken: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// Answered from the recording. Nothing ran.
    Recorded(RecordedResult),
    /// No trustworthy recording: refused (never executed live) and reported.
    Refused(Divergence),
}

pub struct Replayer<'a> {
    cs: &'a ChainStore,
    cassette: CassetteV1,
    replay_run_id: String,
    receipts: HashMap<String, Value>,
    consumed: Vec<bool>,
    steps: u64,
    divergences: Vec<Divergence>,
    expected: Vec<DivergenceKind>,
}

impl<'a> Replayer<'a> {
    /// Open a replay of `cassette` against the recorded chain in `cs`. Only
    /// `EffectsMode::RecordedOnly` exists, so there is no executor to pass.
    pub fn new(cs: &'a ChainStore, cassette: CassetteV1, replay_run_id: &str, _effects: EffectsMode) -> Result<Self> {
        if cassette.schema_id != CASSETTE_SCHEMA_ID { bail!("not a CassetteV1"); }
        let mut me = Self {
            cs, replay_run_id: id_or(Some(replay_run_id), "replay"),
            receipts: HashMap::new(), consumed: vec![false; cassette.entries.len()],
            steps: 0, divergences: vec![], expected: vec![], cassette,
        };
        let run_id = me.cassette.run_id.clone();
        let rep = cs.verify_chain(&run_id)?;
        if let Some(b) = rep.first_break {
            me.push(b.index, "chain", DivergenceKind::ResultHash, None, None,
                    format!("recorded receipt chain does not verify at index {}: {}", b.index, b.reason));
        }
        let all = cs.read_run(&run_id)?;
        let head = all.last().and_then(|r| r["chain"]["content_hash"].as_str()).map(String::from);
        if head.as_deref() != Some(me.cassette.run_receipt_hash.as_str()) {
            let rec = Some(me.cassette.run_receipt_hash.clone());
            me.push(0, "chain", DivergenceKind::ResultHash, rec, head,
                    "cassette run_receipt_hash does not match the recorded chain head".into());
        }
        me.receipts = all.into_iter().map(|r| (receipt_id(&r), r)).collect();
        Ok(me)
    }

    /// Treat divergences of these kinds as expected (verdict EXPECTED_DIVERGENCE).
    pub fn expect(mut self, kinds: &[DivergenceKind]) -> Self { self.expected.extend_from_slice(kinds); self }

    fn push(&mut self, seq: u64, node: &str, kind: DivergenceKind, rec: Option<String>, rep: Option<String>, why: String) -> Divergence {
        let d = Divergence {
            seq, node_id: id_or(Some(node), "unknown"), kind, expected: self.expected.contains(&kind),
            recorded: rec, replayed: rep, explanation: Some(why),
        };
        self.divergences.push(d.clone());
        d
    }

    /// Cross one boundary. Effectful boundaries are answered only from the recording.
    pub fn step(&mut self, s: &ReplayStep) -> StepOutcome {
        let step_no = self.steps;
        self.steps += 1;
        let next = self.consumed.iter().position(|c| !c);
        let matched = self.cassette.entries.iter().enumerate()
            .position(|(i, e)| !self.consumed[i] && e.boundary == s.boundary && e.request_hash == s.request_hash);
        let Some(i) = matched else {
            let rec = next.map(|n| self.cassette.entries[n].request_hash.clone());
            return StepOutcome::Refused(self.push(step_no, &s.node_id.clone(), DivergenceKind::ExtraEntry, rec,
                Some(s.request_hash.clone()),
                format!("no recording for {:?} call {}; refused, not executed", s.boundary, short(&s.request_hash))));
        };
        self.consumed[i] = true;
        let e = self.cassette.entries[i].clone();
        if Some(i) != next || e.node_id != s.node_id {
            let rec = next.map(|n| self.cassette.entries[n].node_id.clone()).or(Some(e.node_id.clone()));
            self.push(e.seq, &e.node_id, DivergenceKind::NodeOrder, rec, Some(s.node_id.clone()),
                      format!("replay step {step_no} reached recorded seq {} out of order", e.seq));
        }
        let Some(r) = self.receipts.get(&e.result_ref).cloned() else {
            let d = self.push(e.seq, &e.node_id, DivergenceKind::MissingEntry, Some(e.result_ref.clone()), None,
                              "recorded receipt is missing from the chain".into());
            return StepOutcome::Refused(d);
        };
        let tampered = content_hash(&r).ok().as_deref() != r["chain"]["content_hash"].as_str();
        let actual = if e.is_effectful() { effect_result_hash(&r) } else { body_result_hash(&r) }.unwrap_or_default();
        if tampered || actual != e.recorded_result_hash {
            let d = self.push(e.seq, &e.node_id, DivergenceKind::ResultHash, Some(e.recorded_result_hash.clone()),
                              Some(actual), if tampered { "recorded receipt body was tampered with" }
                              else { "cassette result hash does not match the recorded receipt" }.into());
            if e.is_effectful() { return StepOutcome::Refused(d); }
        }
        if e.is_effectful() {
            if let Some(k) = &s.idempotency_key {
                let found = self.cs.find_effect(k).ok().flatten().map(|v| receipt_id(&v));
                if found.as_deref() != Some(e.result_ref.as_str()) {
                    let d = self.push(e.seq, &e.node_id, DivergenceKind::ResultHash, Some(e.result_ref.clone()), found,
                                      "idempotency key resolves to a different effect receipt".into());
                    return StepOutcome::Refused(d);
                }
            }
        } else {
            if s.branch.is_some() && s.branch != e.branch_taken {
                let kind = if e.boundary == Boundary::Policy { DivergenceKind::PolicyOutcome } else { DivergenceKind::Branch };
                self.push(e.seq, &e.node_id, kind, e.branch_taken.clone(), s.branch.clone(), "replay took a different branch".into());
            }
            if let Some(h) = &s.result_hash {
                if *h != e.recorded_result_hash {
                    self.push(e.seq, &e.node_id, DivergenceKind::ResultHash, Some(e.recorded_result_hash.clone()),
                              Some(h.clone()), "replay produced a different result".into());
                }
            }
        }
        StepOutcome::Recorded(RecordedResult {
            seq: e.seq, receipt_id: e.result_ref.clone(), result_hash: e.recorded_result_hash.clone(),
            status: r["status"].as_str().map(String::from),
            external_ref: r["external_ref"].as_str().map(String::from),
            branch_taken: e.branch_taken.clone(),
        })
    }

    /// Close the replay: unconsumed recorded entries are `MISSING_ENTRY`.
    pub fn finish(mut self) -> Result<DivergenceReportV1> {
        for i in 0..self.consumed.len() {
            if !self.consumed[i] {
                let e = self.cassette.entries[i].clone();
                self.push(e.seq, &e.node_id, DivergenceKind::MissingEntry, Some(e.request_hash.clone()), None,
                          "recorded boundary was never reached in replay".into());
            }
        }
        let verdict = if self.divergences.is_empty() {
            Verdict::Identical
        } else if self.divergences.iter().all(|d| d.expected) {
            Verdict::ExpectedDivergence
        } else {
            Verdict::UnexpectedDivergence
        };
        let report_id = format!("div_{}", short(&hash_value(&json!({
            "c": self.cassette.cassette_id, "r": self.replay_run_id, "d": serde_json::to_value(&self.divergences)?
        }))?));
        Ok(DivergenceReportV1 {
            schema_id: REPORT_SCHEMA_ID.into(), schema_version: "1.0.0".into(), report_id,
            cassette_id: self.cassette.cassette_id.clone(), replay_run_id: self.replay_run_id.clone(),
            verdict, divergences: self.divergences,
            extensions: Some(Map::from_iter([("x-effects".to_string(), json!("recorded_only")),
                                             ("x-live-effects".to_string(), json!(0))])),
        })
    }
}

/// Replay `steps` (or the cassette's own steps) and report.
pub fn replay_report(cs: &ChainStore, cassette: CassetteV1, steps: Option<Vec<ReplayStep>>, replay_run_id: &str) -> Result<DivergenceReportV1> {
    let steps = steps.unwrap_or_else(|| ReplayStep::from_cassette(&cassette));
    let mut r = Replayer::new(cs, cassette, replay_run_id, EffectsMode::RecordedOnly)?;
    for s in &steps {
        r.step(s);
    }
    r.finish()
}

#[cfg(test)]
mod tests;
