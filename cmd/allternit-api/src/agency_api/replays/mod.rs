//! Replays (`/v1/replays`): re-run a finished run from its recording, with
//! recorded effects only, and test a change against it before it goes live.
//!
//! - **Plain replay**: the run's receipt chain becomes a cassette and the
//!   kernel's recorded-only `Replayer` checks it boundary by boundary
//!   (`allternit_factory_engine::replay`).
//! - **Overrides** vary one thing and compare with the recording:
//!   - `decision_heads` (`{kind: head}` or `{kind: {head, version}}`):
//!     re-decide the run's recorded decisions with a candidate head
//!     (`decide`) and compare with the served and outcome-labelled choices;
//!   - `model_class`: re-run the S2/S3 cognition in another model class
//!     (`sim`), every effect answered from the recording;
//!   - `graph_version`: walk the run's goal through another task-type graph
//!     (registered id, or an inline candidate `ComputeGraphIRV1`), recorded
//!     effects only.
//! - **Batch** (`POST /v1/replays/batch`): the same overrides over N recent
//!   finished runs, with aggregate metrics: the eval set built from real
//!   trajectories. A decisions admin's batch with `decision_heads` is the
//!   replay evidence the flywheel's promotion rule reads
//!   (`decisions::acceptance`).
//!
//! **Effects are recorded_only, always.** The request accepts no other mode;
//! the replay code has no effect path (the kernel `Replayer` takes no
//! executor, `sim` answers effect nodes from the effect journal and refuses
//! the rest); and before a result is stored, [`assert_recorded_only`] checks
//! that the kernel report and the walk both count zero live effects, and
//! fails the replay closed otherwise. Live model calls an override needs go
//! through gizzi-code with tools off, metered on the usage ledger as
//! `surface = replay`, `source = replay`.

pub mod decide;
pub mod sim;
#[cfg(test)]
mod tests;

use super::decisions::{acceptance, flywheel};
use super::store::{new_id, now, RunRecord, EV_REPLAY_BATCH_STATE, EV_REPLAY_STATE};
use super::task_types::{self, TaskType};
use super::{executor, guard, owned, store, ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::AppState;
use allternit_factory_engine::kernel::graph::ComputeGraph;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const OVERRIDE_KEYS: &[&str] = &["decision_heads", "model_class", "graph_version"];
pub const MAX_BATCH_RUNS: usize = 50;
const DEFAULT_BATCH_LIMIT: usize = 20;
const MAX_HEAD_OVERRIDES: usize = 16;
/// Runs a batch selector picks from: finished, not cancelled.
pub(crate) const FINISHED: &[&str] = &["completed", "partial", "failed"];

// ── overrides ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct HeadOverride {
    pub kind: String,
    pub head: &'static dyn flywheel::Head,
}

#[derive(Clone)]
pub enum GraphOverride {
    Registered(&'static TaskType),
    Inline(Value),
}

#[derive(Clone, Default)]
pub struct Overrides {
    pub heads: Vec<HeadOverride>,
    pub model_class: Option<String>,
    pub graph: Option<GraphOverride>,
}

impl Overrides {
    /// The override re-runs cognition (and may need live model calls).
    pub fn cognition(&self) -> bool {
        self.model_class.is_some() || self.graph.is_some()
    }

    pub fn head_for(&self, kind: &str) -> Option<&'static dyn flywheel::Head> {
        self.heads.iter().find(|h| h.kind == kind).map(|h| h.head)
    }

    /// The normalized overrides, echoed on the replay.
    pub fn echo(&self) -> Value {
        let mut o = Map::new();
        if !self.heads.is_empty() {
            let heads: Map<String, Value> =
                self.heads.iter().map(|h| (h.kind.clone(), json!({ "head": h.head.name(), "version": h.head.version() }))).collect();
            o.insert("decision_heads".into(), Value::Object(heads));
        }
        if let Some(c) = &self.model_class {
            o.insert("model_class".into(), json!(c));
        }
        match &self.graph {
            Some(GraphOverride::Registered(t)) => {
                o.insert("graph_version".into(), json!({ "graph_id": t.graph_id, "task_type": t.id, "inline": false }));
            }
            Some(GraphOverride::Inline(g)) => {
                o.insert("graph_version".into(), json!({ "graph_id": g["graph_id"], "inline": true }));
            }
            None => {}
        }
        Value::Object(o)
    }
}

/// A rejected request or a replay that cannot run.
#[derive(Debug)]
pub struct Fail {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub param: Option<String>,
}

impl Fail {
    pub(crate) fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), param: None }
    }
    pub(crate) fn input(param: &str, message: impl Into<String>) -> Self {
        Self { status: 400, code: "ERR_INPUT_INVALID", message: message.into(), param: Some(param.into()) }
    }
    pub(crate) fn unsupported(param: &str, message: impl Into<String>) -> Self {
        Self { status: 400, code: "ERR_INPUT_UNSUPPORTED", message: message.into(), param: Some(param.into()) }
    }
    pub(crate) fn api(self, rid: &RequestId) -> ApiError {
        let family = match self.status {
            400 => "INPUT",
            422 | 409 => "STATE",
            429 => "BUDGET",
            _ => "SYSTEM",
        };
        ApiError::new(self.status, family, self.code, self.message, rid).param(self.param)
    }
    pub(crate) fn json(&self) -> Value {
        json!({ "code": self.code, "message": self.message, "param": self.param })
    }
}

fn valid_kind(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

/// The compiler's model-class rule (`mc.local`, `mc.remote`, and sub-classes).
pub fn valid_class(c: &str) -> bool {
    c == "mc.local" || c == "mc.remote" || c.starts_with("mc.local.") || c.starts_with("mc.remote.")
}

/// Validate `overrides` (absent / null / `{}` = none).
pub fn parse_overrides(v: Option<&Value>) -> Result<Overrides, Fail> {
    let mut ov = Overrides::default();
    let obj = match v {
        None | Some(Value::Null) => return Ok(ov),
        Some(Value::Object(o)) => o,
        Some(_) => return Err(Fail::input("overrides", "overrides must be an object")),
    };
    if let Some(k) = obj.keys().find(|k| !OVERRIDE_KEYS.contains(&k.as_str())) {
        return Err(Fail::unsupported(&format!("overrides.{k}"), format!("unsupported override `{k}` (supported: {})", OVERRIDE_KEYS.join(", "))));
    }
    if let Some(h) = obj.get("decision_heads") {
        let m = h.as_object().filter(|m| !m.is_empty() && m.len() <= MAX_HEAD_OVERRIDES).ok_or_else(|| {
            Fail::input("overrides.decision_heads", format!("decision_heads must map 1-{MAX_HEAD_OVERRIDES} decision kinds to a head"))
        })?;
        for (kind, spec) in m {
            let param = format!("overrides.decision_heads.{kind}");
            if !valid_kind(kind) {
                return Err(Fail::input(&param, "decision kind must be 1-64 characters of [A-Za-z0-9_.-]"));
            }
            let (name, version) = match spec {
                Value::String(s) => (s.as_str(), None),
                Value::Object(o) => {
                    if let Some(x) = o.keys().find(|x| !matches!(x.as_str(), "head" | "version")) {
                        return Err(Fail::unsupported(&param, format!("unknown field `{x}` (a head override is {{head, version}})")));
                    }
                    let name = o.get("head").and_then(Value::as_str).ok_or_else(|| Fail::input(&param, "head is required"))?;
                    let version = match o.get("version") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(Value::Number(n)) => Some(n.to_string()),
                        Some(_) => return Err(Fail::input(&param, "version must be a string")),
                    };
                    (name, version)
                }
                _ => return Err(Fail::input(&param, "a head override is a head name or {head, version}")),
            };
            let head = flywheel::head_by_name(name).ok_or_else(|| {
                let known: Vec<&str> = flywheel::heads().iter().map(|h| h.name()).collect();
                Fail::input(&param, format!("unknown head `{name}` (registered: {})", known.join(", ")))
            })?;
            if !head.applies(kind) {
                return Err(Fail::input(&param, format!("{name} is not registered for kind {kind}")));
            }
            if let Some(v) = version.filter(|v| v != head.version()) {
                return Err(Fail::input(&param, format!("{name} has no version {v} (registered: {})", head.version())));
            }
            ov.heads.push(HeadOverride { kind: kind.clone(), head });
        }
    }
    if let Some(c) = obj.get("model_class") {
        let c = c.as_str().filter(|c| valid_class(c)).ok_or_else(|| {
            Fail::input("overrides.model_class", "model_class must be mc.local, mc.remote or a sub-class (mc.local.<x>, mc.remote.<x>)")
        })?;
        ov.model_class = Some(c.to_string());
    }
    if let Some(g) = obj.get("graph_version") {
        let param = "overrides.graph_version";
        let by_id = |id: &str| -> Result<GraphOverride, Fail> {
            task_types::TASK_TYPES.iter().find(|t| t.graph_id == id).map(GraphOverride::Registered).ok_or_else(|| {
                let known: Vec<&str> = task_types::TASK_TYPES.iter().map(|t| t.graph_id).collect();
                Fail::input(param, format!("unknown graph `{id}` (registered: {})", known.join(", ")))
            })
        };
        ov.graph = Some(match g {
            Value::String(id) => by_id(id)?,
            Value::Object(o) => {
                if let Some(x) = o.keys().find(|x| !matches!(x.as_str(), "graph_id" | "graph")) {
                    return Err(Fail::unsupported(param, format!("unknown field `{x}` (graph_version is a graph id or {{graph_id}} or {{graph}})")));
                }
                match (o.get("graph_id").and_then(Value::as_str), o.get("graph")) {
                    (Some(id), None) => by_id(id)?,
                    (_, Some(inline)) => {
                        if !inline.is_object() {
                            return Err(Fail::input(param, "graph must be a ComputeGraphIRV1 object"));
                        }
                        let parsed = ComputeGraph::from_json(&inline.to_string()).map_err(|e| Fail::input(param, format!("graph is not a ComputeGraphIRV1: {e}")))?;
                        if let Some(id) = o.get("graph_id").and_then(Value::as_str).filter(|id| *id != parsed.graph_id) {
                            return Err(Fail::input(param, format!("graph_id `{id}` does not match the graph's own `{}`", parsed.graph_id)));
                        }
                        GraphOverride::Inline(inline.clone())
                    }
                    _ => return Err(Fail::input(param, "graph_version needs graph_id or graph")),
                }
            }
            _ => return Err(Fail::input(param, "graph_version is a graph id or an object")),
        });
    }
    Ok(ov)
}

/// Reject anything but `effects: recorded_only` (absent = recorded_only).
pub(crate) fn check_effects(obj: &Map<String, Value>) -> Result<(), Fail> {
    match obj.get("effects") {
        None => Ok(()),
        Some(e) if e == "recorded_only" => Ok(()),
        Some(_) => Err(Fail::unsupported("effects", "effects must be recorded_only")),
    }
}

/// Fail closed unless both the kernel replay and the cognition walk report
/// zero live effects.
pub fn assert_recorded_only(kernel_live_effects: Option<u64>, walk_live_effects: u64) -> Result<(), Fail> {
    match kernel_live_effects {
        Some(0) if walk_live_effects == 0 => Ok(()),
        _ => Err(Fail::new(500, "ERR_REPLAY_LIVE_EFFECT", format!(
            "replay is not recorded_only (kernel live effects {kernel_live_effects:?}, walk live effects {walk_live_effects}); result discarded"))),
    }
}

// ── one run ─────────────────────────────────────────────────────────────────

/// Run-level comparison of one replay, for the batch aggregate.
#[derive(Clone, Debug, Default)]
pub struct RunSummary {
    pub verdict: &'static str,
    pub simulated: bool,
    pub source_pass: Option<bool>,
    pub replay_pass: Option<bool>,
    pub live: sim::Spend,
    pub source_regenerated: sim::Spend,
    pub source_tokens: u64,
    pub source_cost_usd: f64,
    pub source_latency_ms: f64,
}

pub struct Outcome {
    pub results: Value,
    pub verdict: &'static str,
    pub rows: Vec<decide::Row>,
    pub summary: RunSummary,
}

fn source_pass(rec: &RunRecord) -> Option<bool> {
    match rec.run["completion"]["status"].as_str() {
        Some("verified") => Some(true),
        Some("failed") | Some("unverified") => Some(false),
        _ => match rec.run["status"].as_str() {
            Some("completed") => Some(true),
            Some("failed") => Some(false),
            _ => None,
        },
    }
}

/// k for pass@k of a kind: the request's, else the kind's bar.
fn k_for(k: Option<u32>, kind: &str) -> u32 {
    k.unwrap_or_else(|| flywheel::bar(kind).replay_k).clamp(1, 50)
}

/// Replay one run with `ov`. Every effect stays recorded_only.
pub async fn replay_one(st: &Arc<AppState>, src: &RunRecord, ov: &Overrides, id: &str, k: Option<u32>) -> Result<Outcome, Fail> {
    let run_id = src.run["id"].as_str().unwrap_or_default().to_string();
    // 1) The receipt chain, through the kernel's recorded-only Replayer.
    let (total, report) = {
    let cs = st.rails.receipts.chain_store().map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
    let cassette = allternit_factory_engine::replay::record_cassette(&cs, &run_id, None, 0)
        .ok()
        .filter(|c| !c.entries.is_empty())
        .ok_or_else(|| Fail::new(422, "ERR_REPLAY_UNRECORDED", "source run has no recorded receipts to replay"))?;
    let total = cassette.entries.len();
    let report = allternit_factory_engine::replay::replay_report(&cs, cassette, None, id).map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
    (total, report)
    };
    let kernel_live = report.extensions.as_ref().and_then(|x| x.get("x-live-effects")).and_then(Value::as_u64);
    let report = serde_json::to_value(report).unwrap_or(Value::Null);
    let mut divergences: Vec<Value> = report["divergences"].as_array().cloned().unwrap_or_default().into_iter().map(|d| {
        let expected = d["expected"].as_bool().unwrap_or(false);
        json!({ "step_ref": format!("{}#{}", d["node_id"].as_str().unwrap_or_default(), d["seq"]),
                "kind": if expected { "expected" } else { "unexpected" }, "type": d["kind"],
                "description": d["explanation"], "source_receipt_id": d["source_receipt_id"], "replay_receipt_id": d["replay_receipt_id"] })
    }).collect();
    let integrity = divergences.len();

    // 2) Cognition, when an override re-runs it, or (B1) when a head
    // override re-decides a decision an S1 node routed on.
    let recorded = {
        let conn = st.db.connect().map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
        decide::load(&conn, &src.owner, &run_id).map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?
    };
    let s1_ids = sim::journaled_decision_ids(&st.db, &run_id).map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
    let s1_redecided = recorded.iter().any(|d| s1_ids.contains(&d.id) && ov.head_for(&d.kind).is_some());
    let walk = if ov.cognition() || s1_redecided { Some(walk_run(st, src, ov).await?) } else { None };
    let covered = walk.as_ref().map(|w| w.decisions_covered.clone()).unwrap_or_default();

    // 3) Decisions.
    let conn = st.db.connect().map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
    let mut decisions = Vec::with_capacity(recorded.len());
    let mut rows = Vec::new();
    for d in &recorded {
        match ov.head_for(&d.kind) {
            Some(head) => {
                let r = decide::redecide(&conn, &src.owner, d, head, k_for(k, &d.kind)).map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
                let (v, row) = decide::compare(d, head, &r);
                // An S1 node's re-decision is reported by the walk, at its node.
                if v["effective_choice"] != json!(d.served) && !covered.contains(&d.id) {
                    divergences.push(json!({ "step_ref": format!("decision:{}", d.id), "kind": "expected", "type": "DECISION",
                        "description": format!("{} chose {} where {} was served", head.name(), v["effective_choice"], json!(d.served)),
                        "source_receipt_id": null, "replay_receipt_id": null }));
                }
                decisions.push(v);
                rows.push(row);
            }
            None => decisions.push(json!({ "decision_id": d.id, "kind": d.kind, "served": { "choice": d.served, "backend": d.backend,
                "latency_ms": d.latency_ms, "cost_usd": d.cost_usd }, "label": d.right(), "candidate": null })),
        }
    }

    drop(conn);
    // 4) recorded_only, asserted (fail closed).
    assert_recorded_only(kernel_live, walk.as_ref().map_or(0, |w| w.live_effects))?;

    let mut summary = RunSummary {
        source_pass: source_pass(src),
        source_tokens: src.run["budget_usage"]["tokens"].as_u64().unwrap_or(0),
        source_cost_usd: src.run["budget_usage"]["cost_usd"].as_f64().unwrap_or(0.0),
        source_latency_ms: src.run["budget_usage"]["duration_ms"].as_f64().unwrap_or(0.0),
        ..Default::default()
    };
    let mut nodes = vec![];
    let mut run_status = json!({ "source": src.run["status"], "replay": "recorded" });
    if let Some(w) = walk {
        summary.simulated = true;
        summary.replay_pass = Some(w.verifier == Some(true) && w.status == "completed");
        summary.live = w.live.clone();
        summary.source_regenerated = w.source_regenerated.clone();
        run_status = json!({ "source": src.run["status"], "replay": w.status, "reason": w.reason });
        divergences.extend(w.divergences);
        nodes = w.nodes;
        run_status["effects"] = json!({ "served_from_recording": w.effects_served, "refused": w.effects_refused, "live": w.live_effects });
    } else {
        summary.replay_pass = summary.source_pass;
        let events = store(st).events(&run_id).await.unwrap_or_default();
        for e in events.iter().filter(|e| e["type"] == "run.progress") {
            let d = &e["data"];
            nodes.push(json!({ "node_id": d["step"], "via": "recorded", "outcome": d["outcome"], "match": true,
                "source": { "outcome": d["outcome"], "tokens": d["tokens"], "duration_ms": d["duration_ms"] } }));
        }
    }
    let verdict = if divergences.is_empty() {
        "equivalent"
    } else if divergences.iter().all(|d| d["kind"] == "expected") {
        "diverged_expected"
    } else {
        "diverged_unexpected"
    };
    summary.verdict = verdict;
    let replay_total = |src_v: f64, regen: f64, live: f64| src_v - regen + live;
    let s = &summary;
    let run_metrics = json!({
        "status": run_status,
        "verifier": { "source": s.source_pass.map(|p| if p { "pass" } else { "fail" }),
                      "replay": s.replay_pass.map(|p| if p { "pass" } else { "fail" }) },
        "tokens": { "source": s.source_tokens, "source_regenerated": s.source_regenerated.tokens, "live": s.live.tokens,
                    "replay": replay_total(s.source_tokens as f64, s.source_regenerated.tokens as f64, s.live.tokens as f64).max(0.0) as u64 },
        "cost_usd": { "source": s.source_cost_usd, "source_regenerated_estimate": s.source_regenerated.cost_usd, "live": s.live.cost_usd,
                      "replay": replay_total(s.source_cost_usd, s.source_regenerated.cost_usd, s.live.cost_usd).max(0.0) },
        "latency_ms": { "source": s.source_latency_ms, "source_regenerated": s.source_regenerated.latency_ms, "live": s.live.latency_ms,
                        "replay": replay_total(s.source_latency_ms, s.source_regenerated.latency_ms, s.live.latency_ms).max(0.0) },
        "live_model_calls": s.live.calls,
    });
    let decision_metrics = (!rows.is_empty()).then(|| decide::metrics_by_kind(&rows, &|kind| k_for(k, kind)));
    let results = json!({
        "object": "replay_results", "replay_id": id, "verdict": verdict, "effects": "recorded_only", "overrides": ov.echo(),
        "decisions_total": total, "decisions_matched": total.saturating_sub(integrity),
        "completion_matched": s.replay_pass == s.source_pass,
        "divergences": divergences, "nodes": nodes, "decisions": decisions,
        "metrics": { "run": run_metrics, "decisions": decision_metrics },
        "has_more": false, "next_cursor": null,
    });
    Ok(Outcome { results, verdict, rows, summary })
}

/// The cognition walk for `model_class` / `graph_version`, and for
/// `decision_heads` that re-decide an S1 node's decision (recordings only).
async fn walk_run(st: &Arc<AppState>, src: &RunRecord, ov: &Overrides) -> Result<sim::SimOut, Fail> {
    let ir = &src.task_ir;
    let task_type = ir["task_type"].as_str().unwrap_or("BUG_FIX");
    if task_type == "BUG_FIX" {
        return Err(Fail::new(422, "ERR_REPLAY_UNSUPPORTED", "model_class and graph_version overrides replay task-type graphs; BUG_FIX runs support decision_heads only in the alpha"));
    }
    let t = task_types::require(task_type).map_err(|e| Fail::new(422, "ERR_REPLAY_UNSUPPORTED", e.to_string()))?;
    let task_id = ir["wih_policy"]["task_id"].as_str().unwrap_or_default().to_string();
    let write_set: Vec<String> = ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let source_graph = task_types::instantiate_for_ir(t, ir, &task_id, &write_set).ok();
    let bad_graph = |e: anyhow::Error| Fail::new(422, "ERR_REPLAY_GRAPH_INVALID", format!("the override graph cannot run this run's goal: {e}"));
    let graph = match &ov.graph {
        None => source_graph.clone().ok_or_else(|| Fail::new(422, "ERR_REPLAY_UNSUPPORTED", "the source run's graph cannot be rebuilt"))?,
        Some(GraphOverride::Registered(g)) => task_types::bind(task_types::graph(g).map_err(bad_graph)?, g.id, g.write_scheme, &task_id, &write_set).map_err(bad_graph)?,
        Some(GraphOverride::Inline(v)) => {
            let g = ComputeGraph::from_json(&v.to_string()).map_err(|e| bad_graph(e.into()))?;
            task_types::bind(g, "candidate", t.write_scheme, &task_id, &write_set).map_err(bad_graph)?
        }
    };
    let tainted = ov.graph.is_some() && source_graph.as_ref().map(serde_json::to_value).transpose().ok().flatten() != serde_json::to_value(&graph).ok();
    let policy = task_types::completion_policy(t).ok_or_else(|| Fail::new(422, "ERR_REPLAY_UNSUPPORTED", format!("{} has no completion contract", t.id)))?;
    let run_id = src.run["id"].as_str().unwrap_or_default();
    let events = store(st).events(run_id).await.map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
    let rec = sim::Recording::load(&st.db, src, &events, source_graph).map_err(|e| Fail::new(500, "ERR_INTERNAL", e.to_string()))?;
    let scripted = executor::scripted();
    let cfg = sim::SimConfig {
        graph: &graph, policy: &policy, goal: ir["goal"].as_str().unwrap_or_default(), write_set: &write_set,
        regen_all: ov.model_class.is_some(), tainted_from_start: tainted, scripted, regenerate: ov.cognition(),
        heads: &ov.heads, db: Some(&st.db), owner: &src.owner, run_id,
    };
    if scripted || !ov.cognition() {
        return Ok(sim::simulate(&cfg, &rec, &sim::Scripted).await);
    }
    let pool = allternit_factory_engine::kernel::router::fetch_model_pool(&executor::gizzi_url(), None)
        .await
        .map_err(|e| Fail::new(503, "ERR_REPLAY_MODEL_UNAVAILABLE", format!("model pool unavailable: {e}")))?;
    let pool = executor::bridge_task_caps(pool, &graph);
    let models = match &ov.model_class {
        Some(c) => json!({ "allow_classes": [c] }),
        None => ir["models"].clone(),
    };
    let (pool, rcfg) = executor::constrain(pool, &models);
    let generative = pool.entries.iter().any(|e| e.cognitive_roles.iter().any(|r| matches!(r, allternit_factory_engine::kernel::router::Role::S2 | allternit_factory_engine::kernel::router::Role::S3)));
    if !generative {
        return Err(Fail::new(422, "ERR_REPLAY_MODEL_CLASS", format!("no generative model is available in class {}", ov.model_class.as_deref().unwrap_or("allowed by the run"))));
    }
    let org = ir["org_id"].as_str().map(str::to_string);
    let ledger = crate::usage_ledger::LedgerCtx::surface("replay").run(run_id, None).tenant(org.as_deref(), Some(&src.owner));
    // Replay and eval live calls count against the same daily caps as runs
    // (global and per org), checked before the walk and before every call.
    let caps = sim::SpendGate::new(store(st), st.db.clone(), org.as_deref().unwrap_or_default(), guard::Limits::from_env().tightened(&ir["rules"]));
    caps.admit().await.map_err(cap_reached)?;
    let live = sim::Live { pool, cfg: rcfg, ledger, caps };
    let out = sim::simulate(&cfg, &rec, &live).await;
    // Fail closed: a cap reached mid-walk discards the partial result.
    match live.caps.tripped() {
        Some(why) => Err(cap_reached(why)),
        None => Ok(out),
    }
}

fn cap_reached(why: String) -> Fail {
    Fail::new(429, "ERR_BUDGET_CAP_REACHED", why)
}

// ── handlers ────────────────────────────────────────────────────────────────

pub(crate) fn replay_object(id: &str, src: &str, ov: &Overrides, batch_id: Option<&str>) -> Value {
    json!({ "id": id, "object": "replay", "source_run_id": src, "effects": "recorded_only", "overrides": ov.echo(), "batch_id": batch_id,
            "status": "running", "verdict": null, "created_at": now(), "finished_at": null,
            "results_url": format!("/v1/replays/{id}/results"), "error": null })
}

pub(crate) fn settle(mut r: Value, out: Result<&Outcome, &Fail>) -> Value {
    r["finished_at"] = json!(now());
    match out {
        Ok(o) => {
            r["status"] = json!("completed");
            r["verdict"] = json!(o.verdict);
            r["results"] = o.results.clone();
        }
        Err(f) => {
            r["status"] = json!("failed");
            r["error"] = f.json();
        }
    }
    r
}

fn without_results(r: &Value) -> Value {
    let mut r = r.clone();
    if let Some(o) = r.as_object_mut() {
        o.remove("results");
    }
    r
}

/// Live model calls make a replay slow: run it in the background.
pub(crate) fn runs_live(ov: &Overrides) -> bool {
    ov.cognition() && !executor::scripted()
}

fn parse_k(obj: &Map<String, Value>) -> Result<Option<u32>, Fail> {
    match obj.get("k") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().filter(|k| (1..=50).contains(k)).map(|k| Some(k as u32)).ok_or_else(|| Fail::input("k", "k must be an integer 1-50")),
    }
}

pub async fn create_replay(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let obj = body.as_object().ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", &rid))?;
    if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "source_run_id" | "effects" | "overrides" | "k")) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), &rid).param(Some(k.clone())));
    }
    check_effects(obj).map_err(|f| f.api(&rid))?;
    let ov = parse_overrides(obj.get("overrides")).map_err(|f| f.api(&rid))?;
    let k = parse_k(obj).map_err(|f| f.api(&rid))?;
    let src_id = obj.get("source_run_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let src = owned(store(&st).load_run(&src_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let id = new_id("rpl");
    let r = replay_object(&id, &src_id, &ov, None);
    if runs_live(&ov) {
        // The chain must be replayable before we accept the job.
        let cs = st.rails.receipts.chain_store().map_err(|e| ApiError::internal(e, &rid))?;
        if allternit_factory_engine::replay::record_cassette(&cs, &src_id, None, 0).map(|c| c.entries.is_empty()).unwrap_or(true) {
            return Err(ApiError::new(422, "STATE", "ERR_REPLAY_UNRECORDED", "source run has no recorded receipts to replay", &rid));
        }
        store(&st).save_object(EV_REPLAY_STATE, &id, &user.user_id, &r).await.map_err(|e| ApiError::internal(e, &rid))?;
        let (st2, owner, id2) = (st.clone(), user.user_id.clone(), id.clone());
        tokio::spawn(async move {
            let out = replay_one(&st2, &src, &ov, &id2, k).await;
            let done = settle(r, out.as_ref());
            if let Err(e) = store(&st2).save_object(EV_REPLAY_STATE, &id2, &owner, &done).await {
                tracing::warn!(replay = %id2, error = %e, "replay result not stored");
            }
        });
        let shown = store(&st).load_object(EV_REPLAY_STATE, &id, &user.user_id).await.ok().flatten().unwrap_or_else(|| json!({}));
        return Ok((StatusCode::ACCEPTED, Json(without_results(&shown))).into_response());
    }
    let out = replay_one(&st, &src, &ov, &id, k).await.map_err(|f| f.api(&rid))?;
    let done = settle(r, Ok(&out));
    store(&st).save_object(EV_REPLAY_STATE, &id, &user.user_id, &done).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::ACCEPTED, Json(without_results(&done))).into_response())
}

pub async fn get_replay(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let r = store(&st).load_object(EV_REPLAY_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("replay", &rid))?;
    Ok(Json(without_results(&r)).into_response())
}

pub async fn get_replay_results(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let r = store(&st).load_object(EV_REPLAY_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("replay", &rid))?;
    if r["status"] != "completed" {
        return Err(ApiError::new(409, "STATE", "ERR_REPLAY_NOT_FINISHED", format!("replay is {}", r["status"].as_str().unwrap_or("unknown")), &rid)
            .with_detail(json!({ "error": r["error"] })));
    }
    Ok(Json(r["results"].clone()).into_response())
}

// ── batch ───────────────────────────────────────────────────────────────────

/// Which runs a batch replays.
#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Runs(Vec<String>),
    Recent { task_type: Option<String>, since: Option<chrono::DateTime<chrono::Utc>>, limit: usize },
}

pub fn parse_selection(obj: &Map<String, Value>) -> Result<Selection, Fail> {
    match (obj.get("runs"), obj.get("selector")) {
        (Some(_), Some(_)) => Err(Fail::input("runs", "give runs or selector, not both")),
        (Some(r), None) => {
            let ids: Vec<String> = r.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            if ids.is_empty() || ids.len() > MAX_BATCH_RUNS || r.as_array().map_or(0, Vec::len) != ids.len() {
                return Err(Fail::input("runs", format!("runs must list 1-{MAX_BATCH_RUNS} run ids")));
            }
            let mut uniq = ids.clone();
            uniq.sort();
            uniq.dedup();
            if uniq.len() != ids.len() {
                return Err(Fail::input("runs", "runs lists a run twice"));
            }
            Ok(Selection::Runs(ids))
        }
        (None, Some(s)) => {
            let o = s.as_object().ok_or_else(|| Fail::input("selector", "selector must be an object"))?;
            if let Some(x) = o.keys().find(|x| !matches!(x.as_str(), "task_type" | "since" | "limit")) {
                return Err(Fail::unsupported(&format!("selector.{x}"), format!("unknown selector field `{x}` (task_type, since, limit)")));
            }
            let task_type = match o.get("task_type") {
                None | Some(Value::Null) => None,
                Some(Value::String(t)) if t == "BUG_FIX" || task_types::get(&t.to_ascii_uppercase()).is_some() => Some(t.to_ascii_uppercase()),
                Some(_) => return Err(Fail::input("selector.task_type", "unknown task_type")),
            };
            let since = match o.get("since") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(chrono::DateTime::parse_from_rfc3339(s).map_err(|_| Fail::input("selector.since", "since must be an RFC 3339 timestamp"))?.with_timezone(&chrono::Utc)),
                Some(_) => return Err(Fail::input("selector.since", "since must be an RFC 3339 timestamp")),
            };
            let limit = match o.get("limit") {
                None | Some(Value::Null) => DEFAULT_BATCH_LIMIT,
                Some(v) => v.as_u64().filter(|l| (1..=MAX_BATCH_RUNS as u64).contains(l)).ok_or_else(|| Fail::input("selector.limit", format!("limit must be 1-{MAX_BATCH_RUNS}")))? as usize,
            };
            Ok(Selection::Recent { task_type, since, limit })
        }
        (None, None) => Err(Fail::input("runs", "give runs (a list of run ids) or selector ({task_type, since, limit})")),
    }
}

/// The owner's most recent finished runs matching the selector.
pub fn select_recent(runs: Vec<RunRecord>, task_type: Option<&str>, since: Option<chrono::DateTime<chrono::Utc>>, limit: usize) -> Vec<RunRecord> {
    let created = |r: &RunRecord| r.run["created_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&chrono::Utc));
    let mut v: Vec<RunRecord> = runs
        .into_iter()
        .filter(|r| FINISHED.contains(&r.run["status"].as_str().unwrap_or_default()))
        .filter(|r| task_type.map_or(true, |t| r.task_ir["task_type"].as_str().unwrap_or("BUG_FIX") == t))
        .filter(|r| since.map_or(true, |s| created(r).is_some_and(|c| c >= s)))
        .collect();
    v.sort_by(|a, b| created(b).cmp(&created(a)));
    v.truncate(limit);
    v
}

fn rate(a: usize, b: usize) -> Option<f64> {
    (b > 0).then(|| a as f64 / b as f64)
}

/// Aggregate run-level metrics of a batch.
pub fn batch_run_metrics(s: &[RunSummary], requested: usize) -> Value {
    let n = s.len();
    let count = |f: &dyn Fn(&RunSummary) -> bool| s.iter().filter(|x| f(x)).count();
    let known = count(&|x| x.source_pass.is_some() && x.replay_pass.is_some());
    let sum = |f: &dyn Fn(&RunSummary) -> f64| s.iter().map(f).sum::<f64>();
    json!({
        "requested": requested,
        "replayed": n,
        "skipped": requested.saturating_sub(n),
        "verdicts": {
            "equivalent": count(&|x| x.verdict == "equivalent"),
            "diverged_expected": count(&|x| x.verdict == "diverged_expected"),
            "diverged_unexpected": count(&|x| x.verdict == "diverged_unexpected"),
        },
        "verifier": {
            "source_pass_rate": rate(count(&|x| x.source_pass == Some(true)), count(&|x| x.source_pass.is_some())),
            "replay_pass_rate": rate(count(&|x| x.replay_pass == Some(true)), count(&|x| x.replay_pass.is_some())),
            "pass_at_1": rate(count(&|x| x.replay_pass == Some(true)), count(&|x| x.replay_pass.is_some())),
            "agreement": rate(count(&|x| x.source_pass.is_some() && x.source_pass == x.replay_pass), known),
        },
        "tokens": { "source": sum(&|x| x.source_tokens as f64) as u64, "source_regenerated": sum(&|x| x.source_regenerated.tokens as f64) as u64,
                    "live": sum(&|x| x.live.tokens as f64) as u64 },
        "cost_usd": { "source": sum(&|x| x.source_cost_usd), "source_regenerated_estimate": sum(&|x| x.source_regenerated.cost_usd), "live": sum(&|x| x.live.cost_usd) },
        "latency_ms": { "source": sum(&|x| x.source_latency_ms), "source_regenerated": sum(&|x| x.source_regenerated.latency_ms), "live": sum(&|x| x.live.latency_ms) },
        "live_model_calls": sum(&|x| x.live.calls as f64) as u64,
        "simulated": count(&|x| x.simulated),
    })
}

/// Replay evidence rows for the flywheel's acceptance rule, one per head
/// override with re-decided decisions.
pub fn evidence_of_batch(batch_id: &str, owner: &str, ov: &Overrides, runs: usize, decisions: &Value) -> Vec<acceptance::Evidence> {
    ov.heads
        .iter()
        .filter_map(|h| {
            let m = &decisions["by_kind"][&h.kind];
            (m["replayed"].as_u64().unwrap_or(0) > 0).then(|| acceptance::Evidence {
                id: batch_id.into(),
                head: h.head.name().into(),
                kind: h.kind.clone(),
                head_version: h.head.version().into(),
                owner: owner.into(),
                created_at: now(),
                runs: runs as u64,
                decisions: m["replayed"].as_u64().unwrap_or(0),
                labelled: m["labelled"].as_u64().unwrap_or(0),
                accuracy: m["accuracy"].as_f64(),
                baseline_accuracy: m["baseline_accuracy"].as_f64(),
                cost_usd: m["cost_usd"].as_f64(),
                baseline_cost_usd: m["baseline_cost_usd"].as_f64(),
                latency_ms: m["latency_ms"].as_f64(),
                baseline_latency_ms: m["baseline_latency_ms"].as_f64(),
                pass_at_1: m["pass_at_1"].as_f64(),
                pass_at_k: m["pass_at_k"].as_f64(),
                k: m["k"].as_u64().unwrap_or(1) as u32,
                deterministic: m["deterministic"].as_bool().unwrap_or(true),
                metrics: m.clone(),
            })
        })
        .collect()
}

async fn run_batch(st: Arc<AppState>, user: AuthUser, batch: Value, runs: Vec<RunRecord>, skipped: Vec<Value>, ov: Overrides, k: Option<u32>) -> Value {
    let id = batch["id"].as_str().unwrap_or_default().to_string();
    let requested = runs.len() + skipped.len();
    let mut skipped = skipped;
    let (mut replays, mut summaries, mut rows) = (vec![], vec![], vec![]);
    for src in runs {
        let src_id = src.run["id"].as_str().unwrap_or_default().to_string();
        let rpl = new_id("rpl");
        let r = replay_object(&rpl, &src_id, &ov, Some(&id));
        let out = replay_one(&st, &src, &ov, &rpl, k).await;
        let done = settle(r, out.as_ref());
        if let Err(e) = store(&st).save_object(EV_REPLAY_STATE, &rpl, &user.user_id, &done).await {
            tracing::warn!(replay = %rpl, error = %e, "replay result not stored");
        }
        match out {
            Ok(o) => {
                replays.push(json!({ "replay_id": rpl, "source_run_id": src_id, "verdict": o.verdict,
                                     "verifier": o.results["metrics"]["run"]["verifier"] }));
                summaries.push(o.summary);
                rows.extend(o.rows);
            }
            Err(f) => skipped.push(json!({ "run_id": src_id, "replay_id": rpl, "reason": f.json() })),
        }
    }
    let decisions = (!rows.is_empty()).then(|| decide::metrics_by_kind(&rows, &|kind| k_for(k, kind)));
    let mut evidence = vec![];
    let mut evidence_note = Value::Null;
    if let Some(d) = &decisions {
        if flywheel::is_flywheel_admin(&user.user_id) {
            let ev = evidence_of_batch(&id, &user.user_id, &ov, summaries.len(), d);
            match st.db.connect() {
                Ok(conn) => {
                    for e in &ev {
                        match acceptance::record(&conn, e) {
                            Ok(()) => evidence.push(json!({ "head": e.head, "kind": e.kind, "evidence_id": e.id,
                                "acceptance": acceptance::judge(e, &flywheel::bar(&e.kind)) })),
                            Err(err) => tracing::warn!(batch = %id, error = %err, "replay evidence not stored"),
                        }
                    }
                }
                Err(err) => tracing::warn!(batch = %id, error = %err, "replay evidence not stored"),
            }
        } else if !ov.heads.is_empty() {
            evidence_note = json!("only a decisions admin's batch replay counts as promotion evidence");
        }
    }
    let mut b = batch;
    let verdict = if summaries.iter().any(|s| s.verdict == "diverged_unexpected") {
        "diverged_unexpected"
    } else if summaries.iter().any(|s| s.verdict == "diverged_expected") {
        "diverged_expected"
    } else if summaries.is_empty() {
        "none"
    } else {
        "equivalent"
    };
    b["status"] = json!("completed");
    b["finished_at"] = json!(now());
    b["verdict"] = json!(verdict);
    b["replays"] = json!(replays);
    b["skipped"] = json!(skipped);
    b["metrics"] = json!({ "runs": batch_run_metrics(&summaries, requested), "decisions": decisions });
    b["evidence"] = json!(evidence);
    b["evidence_note"] = evidence_note;
    b
}

pub async fn create_batch(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let obj = body.as_object().ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", &rid))?;
    if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "runs" | "selector" | "effects" | "overrides" | "k")) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), &rid).param(Some(k.clone())));
    }
    check_effects(obj).map_err(|f| f.api(&rid))?;
    let ov = parse_overrides(obj.get("overrides")).map_err(|f| f.api(&rid))?;
    let k = parse_k(obj).map_err(|f| f.api(&rid))?;
    let sel = parse_selection(obj).map_err(|f| f.api(&rid))?;
    let s = store(&st);
    let (mut runs, mut skipped) = (vec![], vec![]);
    let selector = match &sel {
        Selection::Runs(ids) => {
            for id in ids {
                match s.load_run(id).await.map_err(|e| ApiError::internal(e, &rid))?.filter(|r| r.owner == user.user_id) {
                    Some(r) if FINISHED.contains(&r.run["status"].as_str().unwrap_or_default()) => runs.push(r),
                    Some(r) => skipped.push(json!({ "run_id": id, "reason": { "code": "ERR_REPLAY_NOT_FINISHED", "message": format!("run is {}", r.run["status"].as_str().unwrap_or("unknown")) } })),
                    None => skipped.push(json!({ "run_id": id, "reason": { "code": "ERR_NOT_FOUND", "message": "run not found" } })),
                }
            }
            json!({ "runs": ids })
        }
        Selection::Recent { task_type, since, limit } => {
            let all = s.list_runs(&user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
            runs = select_recent(all, task_type.as_deref(), *since, *limit);
            json!({ "task_type": task_type, "since": since.map(|d| d.to_rfc3339()), "limit": limit })
        }
    };
    let id = new_id("rpb");
    let batch = json!({ "id": id, "object": "replay_batch", "effects": "recorded_only", "overrides": ov.echo(), "selector": selector,
                        "run_count": runs.len(), "status": "running", "verdict": null, "created_at": now(), "finished_at": null,
                        "replays": [], "skipped": skipped, "metrics": null, "evidence": [], "error": null });
    if runs_live(&ov) {
        s.save_object(EV_REPLAY_BATCH_STATE, &id, &user.user_id, &batch).await.map_err(|e| ApiError::internal(e, &rid))?;
        let (st2, b2) = (st.clone(), batch.clone());
        tokio::spawn(async move {
            let owner = user.user_id.clone();
            let id = b2["id"].as_str().unwrap_or_default().to_string();
            let done = run_batch(st2.clone(), user, b2, runs, skipped, ov, k).await;
            if let Err(e) = store(&st2).save_object(EV_REPLAY_BATCH_STATE, &id, &owner, &done).await {
                tracing::warn!(batch = %id, error = %e, "replay batch not stored");
            }
        });
        return Ok((StatusCode::ACCEPTED, Json(batch)).into_response());
    }
    let owner = user.user_id.clone();
    let done = run_batch(st.clone(), user, batch, runs, skipped, ov, k).await;
    s.save_object(EV_REPLAY_BATCH_STATE, &id, &owner, &done).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::ACCEPTED, Json(done)).into_response())
}

pub async fn get_batch(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let b = store(&st).load_object(EV_REPLAY_BATCH_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("replay batch", &rid))?;
    Ok(Json(b).into_response())
}
