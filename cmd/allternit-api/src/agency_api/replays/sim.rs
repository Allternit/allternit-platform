//! Cognition replay: walk a task-type graph again over a run's recording.
//!
//! Used by the `model_class` and `graph_version` overrides. The walk mirrors
//! `executor::generic::run_task_type` node for node, with one difference that
//! is the whole point: **there is no effect path**. The [`Recording`] is the
//! world: model outputs come from the run's journal (or are re-generated live
//! where the override asks for it), WAIT gates from the run's resolved
//! attention items, and every effect node is answered from the run's effect
//! journal. An effect with no recording is refused and reported, never
//! executed; an effect whose request changed (new content) is answered with
//! the recorded result and reported as a divergence. Nothing in this module
//! can reach a connector, the workspace or the receipt chain.
//!
//! B1: an S1 node is answered with its journaled decision
//! (`<run>:<node>:decision.decide:1`), never decided live. Under a
//! `decision_heads` override for the node's kind, the recorded decision is
//! re-decided with the candidate head (`decide`, leave-one-out, nothing
//! written) and the walk follows the effective choice through the node's
//! routes / pass / escalation exactly as the driver would; a changed choice
//! is a DECISION divergence.

use crate::agency_api::executor::generic::{self, effect_args, effect_content, prompt_for, GATE_PRIMITIVE, MAX_STEPS, TASK_WAIT_REASON};
use crate::agency_api::store::RunRecord;
use crate::agency_api::task_types::{evidence_of, s1};
use crate::db::DbHandle;
use allternit_factory_engine::judge::completion::{missing_evidence, CompletionPolicy};
use allternit_factory_engine::kernel::graph::{ComputeGraph, GraphNode};
use allternit_factory_engine::receipts::jcs::{hash_value, sha256_tagged};
use async_trait::async_trait;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// One node of the source run, from its last `run.progress` event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeRun {
    pub outcome: String,
    pub tokens: u64,
    pub duration_ms: u64,
}

/// One recorded effect (the run's effect journal row for the node).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedEffect {
    pub status: String,
    pub result: Option<String>,
    pub args_hash: String,
}

/// What the source run recorded, as the replay's world.
#[derive(Default)]
pub struct Recording {
    /// Node → journaled model output (`model.generate`).
    pub outputs: HashMap<String, String>,
    /// S1 node → its journaled decision (`decision.decide`).
    pub decisions: HashMap<String, s1::Answer>,
    /// Node → its last effect journal row.
    pub effects: HashMap<String, RecordedEffect>,
    /// WAIT node → the person's answer (true = approved).
    pub waits: HashMap<String, bool>,
    /// The source path in order (a node visited twice appears twice).
    pub path: Vec<String>,
    /// Node → its last progress record.
    pub progress: HashMap<String, NodeRun>,
    /// The source run's graph (for node compatibility under `graph_version`).
    pub graph: Option<ComputeGraph>,
    /// The source run's totals: tokens, cost, duration.
    pub tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
}

const EFFECT_TOOLS: &[&str] = &["tool.write_file", "tool.execute"];

impl Recording {
    /// Read the recording of `rec` (journal rows, resolved WAITs, progress events).
    pub fn load(db: &DbHandle, rec: &RunRecord, events: &[Value], graph: Option<ComputeGraph>) -> anyhow::Result<Self> {
        let run_id = rec.run["id"].as_str().unwrap_or_default();
        let mut r = Recording { graph, ..Default::default() };
        let conn = db.connect()?;
        let mut q = conn.prepare(
            "SELECT node_id, tool, status, result, args_hash FROM agency_effect_journal WHERE run_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = q.query_map(params![run_id], |x| {
            Ok((x.get::<_, String>(0)?, x.get::<_, String>(1)?, x.get::<_, String>(2)?, x.get::<_, Option<String>>(3)?, x.get::<_, String>(4)?))
        })?;
        for row in rows {
            let (node, tool, status, result, args_hash) = row?;
            if tool == "model.generate" && status == "committed" {
                let text = result.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()).and_then(|v| v.as_str().map(String::from));
                if let Some(t) = text {
                    r.outputs.insert(node, t);
                }
            } else if tool == "decision.decide" && status == "committed" {
                if let Some(a) = result.as_deref().and_then(|s| serde_json::from_str::<s1::Answer>(s).ok()) {
                    r.decisions.insert(node, a);
                }
            } else if EFFECT_TOOLS.contains(&tool.as_str()) {
                r.effects.insert(node, RecordedEffect { status, result, args_hash });
            }
        }
        for a in &rec.attention {
            if a["reason"] == TASK_WAIT_REASON && a["status"] == "resolved" {
                if let Some(n) = a["node_id"].as_str() {
                    r.waits.insert(n.to_string(), a["resolution"]["outcome"] == "approved");
                }
            }
        }
        for e in events.iter().filter(|e| e["type"] == "run.progress") {
            let d = &e["data"];
            let Some(step) = d["step"].as_str() else { continue };
            r.path.push(step.to_string());
            r.progress.insert(step.to_string(), NodeRun {
                outcome: d["outcome"].as_str().unwrap_or_default().to_string(),
                tokens: d["tokens"].as_u64().unwrap_or(0),
                duration_ms: d["duration_ms"].as_u64().unwrap_or(0),
            });
        }
        let u = &rec.run["budget_usage"];
        r.tokens = u["tokens"].as_u64().unwrap_or(0);
        r.cost_usd = u["cost_usd"].as_f64().unwrap_or(0.0);
        r.duration_ms = u["duration_ms"].as_u64().unwrap_or(0);
        Ok(r)
    }

    /// A recorded output may stand in for `node` only when the source graph
    /// had the same node asking the same thing.
    pub fn compatible(&self, node: &GraphNode) -> bool {
        let Some(g) = &self.graph else { return false };
        let title = |n: &GraphNode| n.extensions.as_ref().and_then(|e| e.get("x-title")).cloned();
        g.node(&node.node_id).is_some_and(|s| {
            s.primitive_id == node.primitive_id
                && s.node_kind == node.node_kind
                && s.cognitive_role == node.cognitive_role
                && s.inputs == node.inputs
                && s.outputs == node.outputs
                && title(s) == title(node)
        })
    }

    /// The source's cost of a node, estimated pro rata by tokens (the run
    /// records cost per run, tokens per step).
    fn est_cost(&self, tokens: u64) -> f64 {
        if self.tokens == 0 { 0.0 } else { self.cost_usd * tokens as f64 / self.tokens as f64 }
    }
}

/// One generation: the text and what it cost.
#[derive(Clone, Debug, Default)]
pub struct Gen {
    pub text: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: f64,
}

/// Cognition for re-generated nodes. Text in, text out: no tools, no effects.
#[async_trait]
pub trait Cognition: Send + Sync {
    async fn generate(&self, node: &GraphNode, prompt: &str) -> Result<Gen, String>;
}

/// Spend of one side of the comparison.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Spend {
    pub calls: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: f64,
}

pub struct SimConfig<'a> {
    pub graph: &'a ComputeGraph,
    pub policy: &'a CompletionPolicy,
    pub goal: &'a str,
    pub write_set: &'a [String],
    /// Re-generate every S2/S3 node (`model_class`); otherwise only nodes the
    /// recording cannot answer for (`graph_version`).
    pub regen_all: bool,
    /// The graph itself differs from the source's: every divergence is explained.
    pub tainted_from_start: bool,
    /// The scripted executor's VERIFY rule (every scripted review passes).
    pub scripted: bool,
    /// Generation is allowed at all (`model_class` / `graph_version`); a
    /// `decision_heads`-only walk serves recordings and refuses the rest.
    pub regenerate: bool,
    /// B1: candidate heads for S1 nodes, re-deciding the run's recorded
    /// decisions (owner `owner`, run `run_id`) from `db`.
    pub heads: &'a [super::HeadOverride],
    pub db: Option<&'a DbHandle>,
    pub owner: &'a str,
    pub run_id: &'a str,
}

#[derive(Debug, Default)]
pub struct SimOut {
    pub nodes: Vec<Value>,
    pub divergences: Vec<Value>,
    /// `completed` | `failed` | `stopped` (an unrecorded WAIT, or the step cap).
    pub status: &'static str,
    pub reason: String,
    /// The verifier's verdict at the completion gate (None: never reached).
    pub verifier: Option<bool>,
    pub live: Spend,
    /// What the source spent on the nodes the replay re-generated.
    pub source_regenerated: Spend,
    /// Effects the replay performed. Always 0: there is no effect path. The
    /// caller asserts it anyway (fail closed).
    pub live_effects: u64,
    pub effects_served: u64,
    pub effects_refused: u64,
    /// Recorded decisions this walk re-decided at their S1 node (their
    /// DECISION divergence, if any, is reported here, by node).
    pub decisions_covered: HashSet<String>,
}

/// The decision ids journaled by a run's S1 nodes.
pub fn journaled_decision_ids(db: &DbHandle, run_id: &str) -> anyhow::Result<HashSet<String>> {
    let conn = db.connect()?;
    let mut q = conn.prepare("SELECT result FROM agency_effect_journal WHERE run_id = ?1 AND tool = 'decision.decide' AND status = 'committed'")?;
    let rows = q.query_map(params![run_id], |x| x.get::<_, Option<String>>(0))?;
    let mut ids = HashSet::new();
    for r in rows {
        if let Some(a) = r?.as_deref().and_then(|s| serde_json::from_str::<s1::Answer>(s).ok()) {
            ids.extend(a.decision_id);
        }
    }
    Ok(ids)
}

/// Re-decide the recorded decision `id` with `head` (leave-one-out, nothing
/// written): the comparison record and the chain's effective choice.
fn redecide_one(cfg: &SimConfig<'_>, head: &dyn crate::agency_api::decisions::flywheel::Head, id: &str) -> Option<(Value, Option<String>)> {
    let conn = cfg.db?.connect().ok()?;
    let d = super::decide::load(&conn, cfg.owner, cfg.run_id).ok()?.into_iter().find(|d| d.id == id)?;
    let r = super::decide::redecide(&conn, cfg.owner, &d, head, 1).ok()?;
    let (v, _) = super::decide::compare(&d, head, &r);
    let eff = v["effective_choice"].as_str().map(str::to_string);
    Some((v, eff))
}

fn divergence(node: &str, ty: &str, expected: bool, description: String) -> Value {
    json!({ "step_ref": node, "kind": if expected { "expected" } else { "unexpected" }, "type": ty,
            "description": description, "source_receipt_id": null, "replay_receipt_id": null })
}

/// Walk `cfg.graph` over `rec`. See the module docs.
pub async fn simulate(cfg: &SimConfig<'_>, rec: &Recording, cog: &dyn Cognition) -> SimOut {
    let g = cfg.graph;
    let mut o = SimOut { status: "stopped", ..Default::default() };
    let mut tainted = cfg.tainted_from_start;
    let mut evidence: Vec<String> = vec![];
    let mut out: HashMap<String, String> = HashMap::new();
    let mut failed_nodes: Vec<String> = vec![];
    let mut authorized = false;
    let mut doc = false;
    let mut visited: Vec<String> = vec![];
    let mut cur = g.entry_nodes.first().cloned().unwrap_or_default();
    let mut steps = 0;
    'walk: loop {
        steps += 1;
        if steps > MAX_STEPS {
            o.reason = "step cap reached".into();
            break;
        }
        let Some(node) = g.node(&cur).cloned() else {
            o.status = "failed";
            o.reason = format!("graph has no node {cur}");
            break;
        };
        visited.push(cur.clone());
        let unmet = node.evidence_required.iter().any(|c| !evidence.iter().any(|e| e.starts_with(&format!("{c}:"))));
        let role = node.cognitive_role.clone().unwrap_or_default();
        let input = node.inputs.iter().filter_map(|i| out.get(i)).cloned().collect::<Vec<_>>().join("\n");
        let src = rec.progress.get(&cur);
        let mut row = json!({ "node_id": cur, "node_kind": node.node_kind, "cognitive_role": role,
            "source": src.map(|s| json!({ "outcome": s.outcome, "tokens": s.tokens, "duration_ms": s.duration_ms })),
            "via": "deterministic", "tokens": 0, "latency_ms": 0.0, "cost_usd": 0.0, "output_matches": null });
        let mut refused: Option<(String, String)> = None; // (type, why): nothing ran
        let (mut branch, mut rejected): (Option<String>, Option<String>) = (None, None);
        let ok: bool = if node.primitive_id == GATE_PRIMITIVE && !node.evidence_required.is_empty() {
            let missing = missing_evidence(cfg.policy, &evidence);
            out.insert("missing".into(), missing.join(", "));
            o.verifier = Some(missing.is_empty());
            row["missing"] = json!(missing);
            missing.is_empty()
        } else if node.node_kind == "WAIT" {
            match rec.waits.get(&cur) {
                Some(open) => {
                    row["via"] = json!("recorded");
                    *open && !unmet
                }
                None => {
                    refused = Some(("UNRECORDED".into(), format!("no recorded answer for WAIT {cur}; the replay stops here (nobody is asked)")));
                    false
                }
            }
        } else if node.node_kind == "POLICY" {
            let declared: HashSet<&String> = cfg.write_set.iter().collect();
            let inside = g.nodes.iter().all(|n| n.write_set.iter().all(|w| declared.contains(w)));
            authorized = inside && !cfg.write_set.is_empty() && !unmet;
            authorized
        } else if !node.write_set.is_empty() {
            if !authorized || unmet {
                false
            } else {
                let content = effect_content(&out, &input);
                let args_hash = hash_value(&effect_args(&cur, &node.write_set, &content)).unwrap_or_default();
                match rec.effects.get(&cur) {
                    None => {
                        o.effects_refused += 1;
                        refused = Some(("UNRECORDED".into(), format!("no recorded effect for {cur}; refused, not executed")));
                        false
                    }
                    Some(e) => {
                        o.effects_served += 1;
                        row["via"] = json!("recorded");
                        row["effect"] = json!({ "served": "recorded", "status": e.status, "request_matches": e.args_hash == args_hash });
                        if e.args_hash != args_hash {
                            o.divergences.push(divergence(&cur, "EFFECT_REQUEST", tainted, format!(
                                "the replay asked {cur} for different content; answered with the recorded result, nothing executed")));
                        }
                        let ok = e.status == "committed";
                        if ok {
                            let r = e.result.clone().unwrap_or_default();
                            if node.primitive_id == "mut.create_file" {
                                doc = true;
                            }
                            for x in &node.outputs {
                                out.insert(x.clone(), r.clone());
                            }
                        }
                        ok
                    }
                }
            }
        } else if s1::is_s1(&node) {
            match s1::spec(&node) {
                Err(e) => {
                    row["error"] = json!(e.to_string());
                    false
                }
                Ok(sp) => {
                    let answer = match rec.decisions.get(&cur) {
                        Some(a) => {
                            row["via"] = json!("recorded");
                            Some(a.clone())
                        }
                        // The scripted executor's rule: the first option.
                        None if cfg.scripted => {
                            row["via"] = json!("scripted");
                            Some(s1::scripted(&sp.options(&out), None, false))
                        }
                        None => None,
                    };
                    match answer {
                        None => {
                            refused = Some(("UNRECORDED".into(), format!("no recorded decision for {cur}; a replay never decides live")));
                            false
                        }
                        Some(mut a) => {
                            let head = cfg.heads.iter().find(|h| h.kind == sp.kind());
                            if let (Some(h), Some(id)) = (head, a.decision_id.clone()) {
                                if let Some((v, eff)) = redecide_one(cfg, h.head, &id) {
                                    o.decisions_covered.insert(id.clone());
                                    row["via"] = json!("candidate_head");
                                    row["candidate"] = v;
                                    if eff != a.choice {
                                        o.divergences.push(divergence(&cur, "DECISION", true, format!(
                                            "{} chose {} at {cur} where {} was served", h.head.name(), json!(eff), json!(a.choice))));
                                        tainted = true;
                                        a = s1::Answer { abstained: eff.is_none(), choice: eff, backend: h.head.name().into(), ..a };
                                    }
                                }
                            }
                            row["decision"] = json!({ "decision_id": a.decision_id, "choice": a.choice, "abstained": a.abstained });
                            match s1::route(g, &cur, &sp, &a) {
                                s1::Route::Commit(t) => {
                                    for x in &node.outputs {
                                        out.insert(x.clone(), a.choice.clone().unwrap_or_default());
                                    }
                                    branch = t;
                                    !unmet
                                }
                                s1::Route::Escalate => false,
                                s1::Route::Reject(c) => {
                                    rejected = Some(c);
                                    false
                                }
                            }
                        }
                    }
                }
            }
        } else if role == "S2" || role == "S3" {
            let regen = cfg.regenerate && (cfg.regen_all || !rec.compatible(&node) || !rec.outputs.contains_key(&cur) && src.is_none());
            let text: Option<Result<String, String>> = if regen {
                let prompt = prompt_for(&node, cfg.goal, &input, None);
                row["via"] = json!("live");
                match cog.generate(&node, &prompt).await {
                    Ok(gen) => {
                        o.live.calls += 1;
                        o.live.tokens += gen.tokens;
                        o.live.cost_usd += gen.cost_usd;
                        o.live.latency_ms += gen.latency_ms;
                        if let Some(s) = src {
                            o.source_regenerated.calls += 1;
                            o.source_regenerated.tokens += s.tokens;
                            o.source_regenerated.latency_ms += s.duration_ms as f64;
                            o.source_regenerated.cost_usd += rec.est_cost(s.tokens);
                        }
                        row["tokens"] = json!(gen.tokens);
                        row["latency_ms"] = json!(gen.latency_ms);
                        row["cost_usd"] = json!(gen.cost_usd);
                        let same = rec.compatible(&node).then(|| rec.outputs.get(&cur).map(|r| *r == gen.text)).flatten();
                        row["output_matches"] = json!(same);
                        row["output_hash"] = json!(sha256_tagged(gen.text.as_bytes()));
                        if same != Some(true) {
                            if same == Some(false) {
                                o.divergences.push(divergence(&cur, "OUTPUT", true, format!("{cur} produced a different output than the recording")));
                            }
                            tainted = true;
                        }
                        Some(Ok(gen.text))
                    }
                    Err(e) => Some(Err(e)),
                }
            } else {
                row["via"] = json!("recorded");
                match rec.outputs.get(&cur) {
                    Some(t) => {
                        row["output_matches"] = json!(true);
                        Some(Ok(t.clone()))
                    }
                    None if src.is_some_and(|s| s.outcome == "failed") => Some(Err("recorded failure".into())),
                    None => None,
                }
            };
            match text {
                Some(Ok(text)) => {
                    let pass = node.node_kind != "VERIFY" || cfg.scripted || text.trim_start().to_ascii_uppercase().starts_with("PASS");
                    for x in &node.outputs {
                        out.insert(x.clone(), text.clone());
                    }
                    pass && !unmet
                }
                Some(Err(e)) => {
                    row["error"] = json!(e);
                    false
                }
                None => {
                    refused = Some(("UNRECORDED".into(), format!("no recorded output for {cur} and the overrides do not re-generate it")));
                    false
                }
            }
        } else {
            let pass = !unmet
                && match node.primitive_id.as_str() {
                    "ver.check_artifact_existence" => {
                        node.inputs.iter().all(|i| out.get(i).is_some_and(|v| !v.trim().is_empty())) && (node.node_id != "R08" || doc)
                    }
                    "ver.check_unresolved_failures" => failed_nodes.is_empty(),
                    "ver.behavior_check" => node.inputs.iter().all(|i| out.get(i).is_some_and(|v| !v.is_empty())),
                    "ver.requirement_check" => {
                        let read = out.get("receipt:sources").cloned().unwrap_or_default();
                        input.match_indices("[src:").all(|(i, _)| input[i..].split(']').next().is_some_and(|c| read.contains(&c[5..])))
                    }
                    _ => true,
                };
            if pass {
                let v = format!("{}:{}", node.primitive_id, sha256_tagged(input.as_bytes()));
                for x in &node.outputs {
                    out.entry(x.clone()).or_insert_with(|| v.clone());
                }
            }
            pass
        };
        let outcome = if refused.is_some() { "refused" } else if ok { "committed" } else { "failed" };
        row["outcome"] = json!(outcome);
        if refused.is_some() {
            row["via"] = json!("refused");
        }
        let matches_source = src.is_some_and(|s| s.outcome == outcome);
        row["match"] = json!(matches_source);
        match src {
            Some(s) if s.outcome != outcome && refused.is_none() => o.divergences.push(divergence(&cur, "OUTCOME", tainted,
                format!("{cur} {outcome} in the replay; the source run recorded {}", s.outcome))),
            None if refused.is_none() => o.divergences.push(divergence(&cur, "EXTRA_NODE", tainted, format!("{cur} ran in the replay but not in the source run"))),
            _ => {}
        }
        o.nodes.push(row);
        if let Some((ty, why)) = refused {
            o.divergences.push(divergence(&cur, &ty, tainted, why.clone()));
            o.status = "stopped";
            o.reason = why;
            break 'walk;
        }
        if let Some(c) = rejected {
            // A confident S1 verdict outside `pass`: the run fails on it, no escalation.
            o.status = "failed";
            o.reason = format!("{cur} decided `{c}`");
            break;
        }
        if ok {
            if let Some(c) = evidence_of(&node) {
                evidence.push(format!("{c}:replay:{cur}"));
            }
            match branch.take().or_else(|| generic::next(g, &cur, "NORMAL")) {
                Some(n) => {
                    cur = n;
                    continue;
                }
                None if g.completion_nodes.contains(&cur) => {
                    o.status = "completed";
                    o.reason = "verified".into();
                }
                None => {
                    o.status = "failed";
                    o.reason = format!("{cur} has no successor and is not a completion node");
                }
            }
            break;
        }
        failed_nodes.push(cur.clone());
        let is_gate = node.primitive_id == GATE_PRIMITIVE;
        let strategy = node.on_failure.strategy.clone();
        match generic::next(g, &cur, "FAILURE") {
            Some(t) if matches!(strategy.as_str(), "FALLBACK" | "ESCALATE") && !is_gate => cur = t,
            Some(t) => {
                visited.push(t.clone());
                let src_t = rec.progress.get(&t);
                o.nodes.push(json!({ "node_id": t, "node_kind": g.node(&t).map(|n| n.node_kind.clone()), "via": "deterministic",
                    "outcome": "committed", "match": src_t.is_some_and(|s| s.outcome == "committed"),
                    "source": src_t.map(|s| json!({ "outcome": s.outcome, "tokens": s.tokens, "duration_ms": s.duration_ms })) }));
                if src_t.is_none() {
                    o.divergences.push(divergence(&t, "EXTRA_NODE", tainted, format!("{t} ran in the replay but not in the source run")));
                }
                o.status = "failed";
                o.reason = if is_gate { format!("verifier: missing evidence for {}", out.get("missing").cloned().unwrap_or_default()) } else { format!("{cur} failed ({strategy}); {t} ran") };
                break;
            }
            None => {
                o.status = "failed";
                o.reason = format!("{cur} failed");
                break;
            }
        }
    }
    let seen: HashSet<&String> = visited.iter().collect();
    let mut reported = HashSet::new();
    for n in &rec.path {
        if !seen.contains(n) && reported.insert(n.clone()) {
            o.divergences.push(divergence(n, "MISSING_NODE", tainted, format!("{n} ran in the source run but not in the replay")));
        }
    }
    o
}

/// Scripted cognition (dev / conformance, `ALLTERNIT_AGENCY_COGNITION=scripted`):
/// the scripted executor's placeholder text, no model call.
pub struct Scripted;

#[async_trait]
impl Cognition for Scripted {
    async fn generate(&self, node: &GraphNode, _prompt: &str) -> Result<Gen, String> {
        Ok(Gen { text: format!("scripted output of {}", node.node_id), ..Default::default() })
    }
}

/// Live cognition through gizzi-code, routed in the overridden model class.
/// Tools off (`complete_ephemeral_usage`): text only, no effect. Every call
/// lands on the usage ledger with `surface = replay`, `source = replay`.
pub struct Live {
    pub pool: allternit_factory_engine::kernel::router::StaticModelPool,
    pub cfg: allternit_factory_engine::kernel::router::RouterConfig,
    pub ledger: crate::usage_ledger::LedgerCtx,
}

#[async_trait]
impl Cognition for Live {
    async fn generate(&self, node: &GraphNode, prompt: &str) -> Result<Gen, String> {
        use allternit_factory_engine::kernel::router::{BudgetLedger, Router};
        let ledger = BudgetLedger { remaining_cost_units: 1.0e9, remaining_wall_ms: None };
        let plan = Router::new(&self.pool, &self.cfg).route(node, &ledger).map_err(|e| format!("route {}: {e}", node.node_id))?;
        let entry = self.pool.entries.iter().find(|e| e.backend_id == plan.backend_id);
        let model = entry
            .and_then(|e| e.extensions.as_ref()?.get("x-model_ref")?.as_str()?.split_once('/'))
            .map(|(p, m)| (p.to_string(), m.to_string()));
        let est = entry.map(|e| e.cost).unwrap_or(0.0);
        let ctx = self.ledger.clone().run(self.ledger.run_id.as_deref().unwrap_or_default(), Some(&node.node_id))
            .tier(node.cognitive_role.as_deref().unwrap_or("S2"));
        let t0 = std::time::Instant::now();
        let (text, usage) = crate::usage_ledger::scope(ctx, crate::gizzi_completion::complete_ephemeral_usage(prompt, Some(generic::SYSTEM), model.as_ref()))
            .await?;
        if text.trim().is_empty() {
            return Err(format!("{}: empty model output", node.node_id));
        }
        Ok(Gen {
            text,
            tokens: usage.tokens,
            cost_usd: if usage.cost_usd > 0.0 { usage.cost_usd } else { est },
            latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }
}
