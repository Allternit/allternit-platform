//! C2: trace monitor. Every run that reaches a terminal state is checked,
//! no sampling: its public event trail, signed receipts, effect journal and
//! decisions go through deterministic checks, each a named finding with a
//! severity and evidence (event seqs, receipt ids, journal keys, decision
//! ids). Optionally one `trace.error` decision (`decisions::decide_value`,
//! in process) asks whether the run contains an error, one option per error
//! class, labelled with the deterministic verdict so a trained head can learn
//! it later (flywheel). No model is required: the decision is off unless
//! `ALLTERNIT_AGENCY_TRACE_DECISION` names a backend chain.
//!
//! Off path: the store's terminal hook spawns the monitor after the terminal
//! `run.status_changed` is durable, so it never delays completion; a failure
//! is logged, never fatal. `GET /v1/runs/:id/findings` reads the stored
//! result (and runs the deterministic checks on demand for a terminal run
//! the hook never saw, e.g. one that finished before an upgrade).
//!
//! Findings of severity high or critical append one `agency.trace.finding`
//! ledger event each (scoped to the run, outside the public event trail).
//! They do not open attention items: the public trail is closed at the
//! terminal event (gap-free `seq`, frozen `RunEventType` enum) and an
//! attention item on a terminal run has no resolution path (an approval
//! re-queues the run).

use super::replays::sim;
use super::store::{now, AgencyStore, RunRecord, TERMINAL};
use super::{decisions, executor, store, task_types};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;
use allternit_factory_engine::kernel::graph::GraphNode;
use async_trait::async_trait;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub const MONITOR_VERSION: &str = "trace-monitor.v1";
/// Stored findings of one run (ledger object, keyed by run id and owner).
pub const EV_FINDINGS: &str = super::store::EV_FINDINGS_STATE;
/// One ledger event per finding of severity high or critical.
pub const EV_TRACE_FINDING: &str = "agency.trace.finding";
/// `0` turns the monitor's terminal hook off (the endpoint still checks on demand).
pub const ENABLE_ENV: &str = "ALLTERNIT_AGENCY_TRACE_MONITOR";
/// Unset/`0`/`off`: no decision. `1`/`on`: the configured decision chain.
/// Otherwise a comma list of backend names (`head` = learned heads only, no model).
pub const DECISION_ENV: &str = "ALLTERNIT_AGENCY_TRACE_DECISION";
pub const DECISION_KIND: &str = "trace.error";

/// The journal tool of a B1 S1 decision node's answer.
pub const S1_TOOL: &str = "decision.decide";
/// S1 answers that escalate as a fraction of all S1 answers at which the run is an escalation storm.
pub const S1_ESCALATION_SHARE: f64 = 0.5;
/// Effects that need a preceding policy allow (they mutate the declared resources).
pub const MUTATING_TOOLS: &[&str] = &["tool.write_file", "tool.execute", "tool.fs_write"];
/// A step run this many times or more is a loop.
pub const LOOP_STEP_REPEATS: usize = 4;
/// The same tool with the same arguments this many times or more is a loop.
pub const LOOP_IDENTICAL_CALLS: usize = 3;
/// Attention requests or S3 escalations in one run that make a storm.
pub const STORM: usize = 3;
/// A budget dimension used past this fraction is near exhaustion.
pub const BUDGET_NEAR: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "info" => Severity::Info,
            "low" => Severity::Low,
            "medium" => Severity::Medium,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => return None,
        })
    }
}

/// The deterministic checks, in the order they run. `(id, title)`; the ids
/// are also the options of the `trace.error` decision.
pub const CHECKS: &[(&str, &str)] = &[
    ("effect_without_policy_allow", "An effect ran without a preceding policy allow"),
    ("verifier_pass_missing_evidence", "The verifier passed with evidence missing"),
    ("completed_without_verification", "The run completed without a passing verification"),
    ("effect_content_divergence", "An effect's content differs from the model output that fed it"),
    ("effect_outcome_unresolved", "An effect's outcome was never settled"),
    ("repeated_steps", "Repeated identical steps or tool calls (loop)"),
    ("escalation_storm", "Escalation or attention storm"),
    ("budget_near_exhaustion", "Budget near exhaustion"),
    ("unanswered_attention", "Attention requests left unanswered"),
    ("low_confidence_decision_served", "A decision below its confidence threshold was served"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub check: &'static str,
    pub severity: Severity,
    pub detail: String,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    pub event_seqs: Vec<i64>,
    pub receipt_ids: Vec<String>,
    pub journal_keys: Vec<String>,
    pub decision_ids: Vec<String>,
    pub attention_ids: Vec<String>,
    pub nodes: Vec<String>,
}

impl Evidence {
    fn json(&self) -> Value {
        let mut e = self.clone();
        e.event_seqs.sort_unstable();
        e.event_seqs.dedup();
        for v in [&mut e.receipt_ids, &mut e.journal_keys, &mut e.decision_ids, &mut e.attention_ids, &mut e.nodes] {
            v.sort();
            v.dedup();
        }
        json!({ "event_seqs": e.event_seqs, "receipt_ids": e.receipt_ids, "journal_keys": e.journal_keys,
                "decision_ids": e.decision_ids, "attention_ids": e.attention_ids, "nodes": e.nodes })
    }
}

impl Finding {
    pub fn title(&self) -> &'static str {
        CHECKS.iter().find(|(id, _)| *id == self.check).map(|(_, t)| *t).unwrap_or("")
    }
    pub fn json(&self, run_id: &str) -> Value {
        let ev = self.evidence.json();
        let id = allternit_factory_engine::receipts::jcs::sha256_tagged(format!("{run_id}|{}|{ev}", self.check).as_bytes());
        let id = format!("fnd_{}", id.rsplit(':').next().unwrap_or(&id).chars().take(20).collect::<String>());
        json!({ "id": id, "object": "finding", "check": self.check, "severity": self.severity.as_str(), "title": self.title(),
                "detail": self.detail, "evidence": ev })
    }
}

/// One effect journal row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JournalRow {
    pub key: String,
    pub node: String,
    pub tool: String,
    pub status: String,
    pub args_hash: String,
    /// The journaled value (S1 answers, model outputs); effects' results are not read.
    pub result: Option<String>,
}

/// One stored decision of the run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecisionRow {
    pub id: String,
    pub kind: String,
    pub choice: Option<String>,
    pub confidence: f64,
    pub abstained: bool,
    pub backend: String,
}

/// Everything the checks read, loaded once.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub run: Value,
    pub attention: Vec<Value>,
    pub events: Vec<Value>,
    pub receipts: Vec<Value>,
    pub journal: Vec<JournalRow>,
    pub decisions: Vec<DecisionRow>,
    /// B1 S1 decision nodes of the run's graph: node id -> decision kind.
    pub s1_kinds: HashMap<String, String>,
    /// Divergences of the recorded re-walk (task-type runs): `EFFECT_REQUEST`
    /// means an effect asked for other content than its recorded inputs give.
    pub divergences: Vec<Value>,
}

fn seq(e: &Value) -> i64 {
    e["seq"].as_i64().unwrap_or(0)
}

fn receipt_by_id<'a>(t: &'a Trace, id: &str) -> Option<&'a Value> {
    t.receipts.iter().find(|r| r["chain"]["receipt_id"] == id)
}

fn receipt_events<'a>(t: &'a Trace, ty: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    t.events.iter().filter(move |e| e["type"] == "receipt.appended" && e["data"]["receipt_type"] == ty)
}

// ── the checks ──────────────────────────────────────────────────────────────

/// One journaled S1 answer (`decision.decide`, B1 decision nodes).
#[derive(Debug, Clone, PartialEq)]
pub struct S1Answer {
    pub key: String,
    pub node: String,
    pub kind: String,
    pub decision_id: Option<String>,
    pub choice: Option<String>,
    pub confidence: f64,
    pub abstained: bool,
    pub backend: String,
}

/// Journal kinds that record values (model output, S1 answers, checkpoints), not effects.
pub const VALUE_KINDS: &[&str] = &["model.generate", S1_TOOL, "checkpoint"];

pub fn is_effect(tool: &str) -> bool {
    !VALUE_KINDS.contains(&tool)
}

/// The run's S1 answers, in journal order.
pub fn s1_answers(t: &Trace) -> Vec<S1Answer> {
    t.journal.iter().filter(|r| r.tool == S1_TOOL && r.status == "committed").filter_map(|r| {
        let v: Value = serde_json::from_str(r.result.as_deref()?).ok()?;
        Some(S1Answer {
            key: r.key.clone(), node: r.node.clone(),
            kind: t.s1_kinds.get(&r.node).cloned().unwrap_or_else(|| "default".into()),
            decision_id: v["decision_id"].as_str().map(String::from), choice: v["choice"].as_str().map(String::from),
            confidence: v["confidence"].as_f64().unwrap_or(0.0), abstained: v["abstained"] == true,
            backend: v["backend"].as_str().unwrap_or_default().to_string(),
        })
    }).collect()
}

/// A committed mutating effect needs a policy allow earlier in the trail. A
/// policy receipt the chain says is a deny does not count; one the monitor
/// cannot read is given the benefit of the doubt.
pub fn check_effect_without_policy(t: &Trace) -> Option<Finding> {
    let allows: Vec<(i64, String)> = receipt_events(t, "policy_decision")
        .filter_map(|e| {
            let rid = e["data"]["receipt_id"].as_str().unwrap_or_default().to_string();
            let deny = receipt_by_id(t, &rid).is_some_and(|r| !r["decision"].as_str().unwrap_or_default().eq_ignore_ascii_case("allow"));
            (!deny).then_some((seq(e), rid))
        })
        .collect();
    let mut ev = Evidence::default();
    let mut n = 0;
    for row in t.journal.iter().filter(|r| r.status == "committed" && MUTATING_TOOLS.contains(&r.tool.as_str())) {
        let eff = receipt_events(t, "effect").find(|e| e["data"]["step"] == row.node.as_str());
        let at = eff.map(seq);
        let ok = allows.iter().any(|(s, _)| at.map_or(true, |a| *s < a));
        if !ok {
            n += 1;
            ev.journal_keys.push(row.key.clone());
            ev.nodes.push(row.node.clone());
            if let Some(e) = eff {
                ev.event_seqs.push(seq(e));
                ev.receipt_ids.extend(e["data"]["receipt_id"].as_str().map(String::from));
            }
        }
    }
    // Policy receipts that said deny are evidence too.
    for e in receipt_events(t, "policy_decision") {
        let rid = e["data"]["receipt_id"].as_str().unwrap_or_default();
        if n > 0 && !allows.iter().any(|(_, r)| r == rid) {
            ev.receipt_ids.push(rid.to_string());
            ev.event_seqs.push(seq(e));
        }
    }
    (n > 0).then(|| Finding {
        check: "effect_without_policy_allow",
        severity: Severity::Critical,
        detail: format!("{n} committed mutating effect(s) with no policy allow before them"),
        evidence: ev,
    })
}

pub fn check_verifier_pass_missing(t: &Trace) -> Option<Finding> {
    let mut ev = Evidence::default();
    let mut why = vec![];
    for e in t.events.iter().filter(|e| e["type"] == "verification.completed" && e["data"]["result"] == "pass") {
        if e["data"]["missing"].as_array().is_some_and(|m| !m.is_empty()) {
            ev.event_seqs.push(seq(e));
            ev.receipt_ids.extend(e["data"]["receipt_id"].as_str().map(String::from));
            why.push("a passing verification event lists missing evidence".to_string());
        }
    }
    for r in t.receipts.iter().filter(|r| r["type"] == "verification" && r["result"].as_str().is_some_and(|x| x.eq_ignore_ascii_case("pass"))) {
        if r["missing"].as_array().is_some_and(|m| !m.is_empty()) {
            ev.receipt_ids.extend(r["chain"]["receipt_id"].as_str().map(String::from));
            why.push("a PASS verification receipt lists missing evidence".to_string());
        }
    }
    if t.run["completion"]["status"] == "verified" {
        let bad: Vec<String> = t.run["completion"]["criteria"].as_array().into_iter().flatten()
            .filter(|c| c["result"] != "pass")
            .filter_map(|c| c["criterion"].as_str().map(String::from))
            .collect();
        if !bad.is_empty() {
            why.push(format!("completion is verified but criteria {} did not pass", bad.join(", ")));
            ev.nodes.extend(bad);
        }
    }
    why.dedup();
    (!why.is_empty()).then(|| Finding { check: "verifier_pass_missing_evidence", severity: Severity::High, detail: why.join("; "), evidence: ev })
}

pub fn check_completed_unverified(t: &Trace) -> Option<Finding> {
    if t.run["status"] != "completed" {
        return None;
    }
    let event_pass = t.events.iter().any(|e| e["type"] == "verification.completed" && e["data"]["result"] == "pass");
    let receipt_pass = t.receipts.iter().any(|r| r["type"] == "verification" && r["result"].as_str().is_some_and(|x| x.eq_ignore_ascii_case("pass")));
    (!event_pass && !receipt_pass).then(|| Finding {
        check: "completed_without_verification",
        severity: Severity::High,
        detail: "the run completed but no verification passed (completion is verifier-owned)".into(),
        evidence: Evidence { event_seqs: t.events.iter().filter(|e| e["type"] == "run.status_changed" && e["data"]["to"] == "completed").map(seq).collect(), ..Default::default() },
    })
}

pub fn check_effect_divergence(t: &Trace) -> Option<Finding> {
    let nodes: Vec<String> = t.divergences.iter().filter(|d| d["type"] == "EFFECT_REQUEST").filter_map(|d| d["step_ref"].as_str().map(String::from)).collect();
    if nodes.is_empty() {
        return None;
    }
    let mut ev = Evidence { nodes: nodes.clone(), ..Default::default() };
    for n in &nodes {
        ev.journal_keys.extend(t.journal.iter().filter(|r| &r.node == n && MUTATING_TOOLS.contains(&r.tool.as_str())).map(|r| r.key.clone()));
        for e in receipt_events(t, "effect").filter(|e| e["data"]["step"] == n.as_str()) {
            ev.event_seqs.push(seq(e));
            ev.receipt_ids.extend(e["data"]["receipt_id"].as_str().map(String::from));
        }
    }
    Some(Finding {
        check: "effect_content_divergence",
        severity: Severity::High,
        detail: format!("effect content at {} does not match the recorded model output feeding it", nodes.join(", ")),
        evidence: ev,
    })
}

pub fn check_effect_unresolved(t: &Trace) -> Option<Finding> {
    let rows: Vec<&JournalRow> = t.journal.iter().filter(|r| r.status == "prepared").collect();
    (!rows.is_empty()).then(|| Finding {
        check: "effect_outcome_unresolved",
        severity: Severity::High,
        detail: format!("{} effect(s) prepared but never committed or failed", rows.len()),
        evidence: Evidence { journal_keys: rows.iter().map(|r| r.key.clone()).collect(), nodes: rows.iter().map(|r| r.node.clone()).collect(), ..Default::default() },
    })
}

pub fn check_repeated_steps(t: &Trace) -> Option<Finding> {
    let mut ev = Evidence::default();
    let (mut hit, mut high) = (false, false);
    let mut steps: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for e in t.events.iter().filter(|e| e["type"] == "run.progress") {
        if let Some(s) = e["data"]["step"].as_str() {
            steps.entry(s.to_string()).or_default().push(seq(e));
        }
    }
    let mut why = vec![];
    for (s, seqs) in &steps {
        if seqs.len() >= LOOP_STEP_REPEATS {
            hit = true;
            high |= seqs.len() >= 2 * LOOP_STEP_REPEATS;
            why.push(format!("{s} ran {} times", seqs.len()));
            ev.nodes.push(s.clone());
            ev.event_seqs.extend(seqs);
        }
    }
    let mut calls: BTreeMap<(String, String), Vec<&JournalRow>> = BTreeMap::new();
    for r in t.journal.iter().filter(|r| !r.args_hash.is_empty()) {
        calls.entry((r.tool.clone(), r.args_hash.clone())).or_default().push(r);
    }
    for ((tool, _), rows) in &calls {
        if rows.len() >= LOOP_IDENTICAL_CALLS {
            hit = true;
            high |= rows.len() >= 2 * LOOP_IDENTICAL_CALLS;
            why.push(format!("{tool} called {} times with identical arguments", rows.len()));
            ev.journal_keys.extend(rows.iter().map(|r| r.key.clone()));
            ev.nodes.extend(rows.iter().map(|r| r.node.clone()));
        }
    }
    hit.then(|| Finding {
        check: "repeated_steps",
        severity: if high { Severity::High } else { Severity::Medium },
        detail: why.join("; "),
        evidence: ev,
    })
}

pub fn check_escalation_storm(t: &Trace) -> Option<Finding> {
    let att: Vec<&Value> = t.events.iter().filter(|e| e["type"] == "attention.requested").collect();
    let s3: Vec<&Value> = t.events.iter().filter(|e| e["type"] == "run.progress" && e["data"]["cognitive_role"] == "S3").collect();
    // B1: S1 decision nodes that abstained (escalated to their S2/S3 twin).
    let s1 = s1_answers(t);
    let s1_up: Vec<&S1Answer> = s1.iter().filter(|a| a.abstained).collect();
    let s1_storm = s1_up.len() >= 2 && s1_up.len() as f64 >= S1_ESCALATION_SHARE * s1.len() as f64;
    let n = att.len().max(s3.len()).max(s1_up.len());
    if n < STORM && !s1_storm {
        return None;
    }
    let mut ev = Evidence::default();
    ev.event_seqs.extend(att.iter().map(|e| seq(e)).chain(s3.iter().map(|e| seq(e))));
    ev.attention_ids.extend(att.iter().filter_map(|e| e["data"]["attention"]["id"].as_str().map(String::from)));
    ev.nodes.extend(s3.iter().filter_map(|e| e["data"]["step"].as_str().map(String::from)));
    ev.nodes.extend(s1_up.iter().map(|a| a.node.clone()));
    ev.journal_keys.extend(s1_up.iter().map(|a| a.key.clone()));
    ev.decision_ids.extend(s1_up.iter().filter_map(|a| a.decision_id.clone()));
    Some(Finding {
        check: "escalation_storm",
        severity: if n >= 2 * STORM { Severity::High } else { Severity::Medium },
        detail: format!("{} attention request(s), {} S3 escalation(s) and {} of {} S1 decision(s) escalated in one run", att.len(), s3.len(), s1_up.len(), s1.len()),
        evidence: ev,
    })
}

pub fn check_budget(t: &Trace) -> Option<Finding> {
    let (b, u) = (&t.run["budget"], &t.run["budget_usage"]);
    let mut worst: Option<(f64, &str)> = None;
    for (lim, used) in [("max_cost_usd", "cost_usd"), ("max_seconds", "seconds"), ("max_steps", "steps")] {
        if let Some(l) = b[lim].as_f64().filter(|l| *l > 0.0) {
            let f = u[used].as_f64().unwrap_or(0.0) / l;
            if worst.map_or(true, |(w, _)| f > w) {
                worst = Some((f, lim));
            }
        }
    }
    let halted = u["spend_halted"] == true;
    let (frac, dim) = worst.unwrap_or((0.0, ""));
    if frac < BUDGET_NEAR && !halted {
        return None;
    }
    let ev = Evidence { event_seqs: t.events.iter().filter(|e| e["type"] == "budget.threshold").map(seq).collect(), ..Default::default() };
    Some(Finding {
        check: "budget_near_exhaustion",
        severity: if frac >= 1.0 || halted { Severity::High } else { Severity::Medium },
        detail: if halted { format!("spend was halted ({dim} at {:.0}%)", frac * 100.0) } else { format!("{dim} at {:.0}% of its budget", frac * 100.0) },
        evidence: ev,
    })
}

pub fn check_unanswered_attention(t: &Trace) -> Option<Finding> {
    let open: Vec<&Value> = t.attention.iter().filter(|a| a["status"] == "open").collect();
    if open.is_empty() {
        return None;
    }
    let ids: Vec<String> = open.iter().filter_map(|a| a["id"].as_str().map(String::from)).collect();
    let seqs = t.events.iter().filter(|e| e["type"] == "attention.requested" && e["data"]["attention"]["id"].as_str().is_some_and(|i| ids.iter().any(|x| x == i))).map(seq).collect();
    Some(Finding {
        check: "unanswered_attention",
        severity: if t.run["status"] == "completed" { Severity::High } else { Severity::Medium },
        detail: format!("{} attention request(s) still open when the run ended {}", ids.len(), t.run["status"].as_str().unwrap_or("")),
        evidence: Evidence { attention_ids: ids, event_seqs: seqs, ..Default::default() },
    })
}

pub fn check_low_confidence(t: &Trace) -> Option<Finding> {
    let low: Vec<&DecisionRow> = t.decisions.iter()
        .filter(|d| d.kind != DECISION_KIND && d.choice.is_some() && !d.abstained && d.confidence < decisions::threshold(&d.kind))
        .collect();
    // B1: S1 decision nodes that acted on an answer under their kind's
    // threshold (as journaled: what the run actually acted on).
    let s1: Vec<S1Answer> = s1_answers(t).into_iter()
        .filter(|a| a.choice.is_some() && !a.abstained && a.confidence < decisions::threshold(&a.kind))
        .filter(|a| !a.decision_id.as_ref().is_some_and(|id| low.iter().any(|d| &d.id == id)))
        .collect();
    if low.is_empty() && s1.is_empty() {
        return None;
    }
    let mut why: Vec<String> = low.iter().map(|d| format!("{} ({}) served at confidence {:.2} < {:.2} by {}", d.id, d.kind, d.confidence, decisions::threshold(&d.kind), d.backend)).collect();
    why.extend(s1.iter().map(|a| format!("S1 node {} ({}) acted at confidence {:.2} < {:.2} by {}", a.node, a.kind, a.confidence, decisions::threshold(&a.kind), a.backend)));
    let mut ev = Evidence { decision_ids: low.iter().map(|d| d.id.clone()).collect(), ..Default::default() };
    ev.decision_ids.extend(s1.iter().filter_map(|a| a.decision_id.clone()));
    ev.journal_keys.extend(s1.iter().map(|a| a.key.clone()));
    ev.nodes.extend(s1.iter().map(|a| a.node.clone()));
    Some(Finding { check: "low_confidence_decision_served", severity: Severity::Medium, detail: why.join("; "), evidence: ev })
}

/// Every deterministic check over `t`, most severe first (stable within a severity).
pub fn run_checks(t: &Trace) -> Vec<Finding> {
    let checks: [fn(&Trace) -> Option<Finding>; 10] = [
        check_effect_without_policy,
        check_verifier_pass_missing,
        check_completed_unverified,
        check_effect_divergence,
        check_effect_unresolved,
        check_repeated_steps,
        check_escalation_storm,
        check_budget,
        check_unanswered_attention,
        check_low_confidence,
    ];
    let mut f: Vec<Finding> = checks.iter().filter_map(|c| c(t)).collect();
    f.sort_by(|a, b| b.severity.cmp(&a.severity));
    f
}

/// Counts the checks looked at (also the `trace.error` decision's context).
pub fn signals(t: &Trace) -> Value {
    let count = |ty: &str| t.events.iter().filter(|e| e["type"] == ty).count();
    json!({
        "events": t.events.len(), "receipts": t.receipts.len(), "progress": count("run.progress"),
        "attention_requested": count("attention.requested"), "verifications": count("verification.completed"),
        "effects": t.journal.iter().filter(|r| is_effect(&r.tool)).count(),
        "s1_decisions": s1_answers(t).len(),
        "model_outputs": t.journal.iter().filter(|r| r.tool == "model.generate").count(),
        "decisions": t.decisions.len(), "divergences": t.divergences.len(),
    })
}

/// The stored findings object.
pub fn report(run_id: &str, t: &Trace, findings: &[Finding], decision: Value) -> Value {
    let mut by: BTreeMap<&str, usize> = ["critical", "high", "medium", "low", "info"].iter().map(|s| (*s, 0)).collect();
    for f in findings {
        *by.entry(f.severity.as_str()).or_default() += 1;
    }
    json!({
        "object": "run_findings", "run_id": run_id, "monitor": MONITOR_VERSION, "run_status": t.run["status"], "checked_at": now(),
        "summary": { "total": findings.len(), "by_severity": by, "max_severity": findings.first().map(|f| f.severity.as_str()),
                     "checks_run": CHECKS.len() },
        "data": findings.iter().map(|f| f.json(run_id)).collect::<Vec<_>>(),
        "signals": signals(t), "decision": decision,
    })
}

/// Summary for the graph view: counts only.
pub fn summary_of(stored: &Value) -> Value {
    json!({ "total": stored["summary"]["total"], "max_severity": stored["summary"]["max_severity"],
            "url": format!("/v1/runs/{}/findings", stored["run_id"].as_str().unwrap_or_default()) })
}

// ── loading ─────────────────────────────────────────────────────────────────

pub fn load_journal(db: &DbHandle, run_id: &str) -> anyhow::Result<Vec<JournalRow>> {
    let c = db.connect()?;
    let mut q = c.prepare("SELECT idempotency_key, COALESCE(node_id, ''), tool, status, args_hash, CASE WHEN tool = ?2 THEN result END
         FROM agency_effect_journal WHERE run_id = ?1 ORDER BY created_at, rowid")?;
    let rows = q.query_map(params![run_id, S1_TOOL], |r| Ok(JournalRow { key: r.get(0)?, node: r.get(1)?, tool: r.get(2)?, status: r.get(3)?, args_hash: r.get(4)?, result: r.get(5)? }))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn load_decisions(db: &DbHandle, owner: &str, run_id: &str) -> anyhow::Result<Vec<DecisionRow>> {
    let c = db.connect()?;
    let mut q = c.prepare(
        "SELECT id, kind, choice, confidence, abstained, backend FROM decisions WHERE owner = ?1 AND (session_id = ?2 OR task = ?2) ORDER BY created_at, id",
    )?;
    let rows = q.query_map(params![owner, run_id], |r| Ok(DecisionRow {
        id: r.get(0)?, kind: r.get(1)?, choice: r.get(2)?, confidence: r.get(3)?, abstained: r.get(4)?, backend: r.get(5)?,
    }))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Cognition for the recorded re-walk: no model, ever.
struct NoModel;

#[async_trait]
impl sim::Cognition for NoModel {
    async fn generate(&self, node: &GraphNode, _prompt: &str) -> Result<sim::Gen, String> {
        Err(format!("{}: not recorded (the monitor calls no model)", node.node_id))
    }
}

/// Walk a task-type run's own graph over its own recording (no model, no
/// effect): an `EFFECT_REQUEST` divergence means the content an effect sent
/// is not what the recorded outputs feeding it give.
async fn recorded_walk(db: &DbHandle, rec: &RunRecord, events: &[Value]) -> Vec<Value> {
    let ir = &rec.task_ir;
    let Some(t) = ir["task_type"].as_str().filter(|t| *t != "BUG_FIX").and_then(task_types::get) else { return vec![] };
    let task_id = ir["wih_policy"]["task_id"].as_str().unwrap_or_default();
    let write_set: Vec<String> = ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
    let (Ok(graph), Some(policy)) = (task_types::instantiate(t, task_id, &write_set), task_types::completion_policy(t)) else { return vec![] };
    let Ok(recording) = sim::Recording::load(db, rec, events, Some(graph.clone())) else { return vec![] };
    let cfg = sim::SimConfig {
        graph: &graph, policy: &policy, goal: ir["goal"].as_str().unwrap_or_default(), write_set: &write_set,
        regen_all: false, tainted_from_start: false, scripted: executor::scripted(), regenerate: false,
        heads: &[], db: Some(db), owner: &rec.owner, run_id: rec.run["id"].as_str().unwrap_or_default(),
    };
    sim::simulate(&cfg, &recording, &NoModel).await.divergences
}

/// S1 decision nodes of a task-type run's graph and their decision kinds.
fn s1_kinds(rec: &RunRecord) -> HashMap<String, String> {
    let ir = &rec.task_ir;
    let Some(t) = ir["task_type"].as_str().and_then(task_types::get) else { return HashMap::new() };
    let write_set: Vec<String> = ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
    let Ok(g) = task_types::instantiate(t, ir["wih_policy"]["task_id"].as_str().unwrap_or_default(), &write_set) else { return HashMap::new() };
    g.nodes.iter().filter(|n| task_types::s1::is_s1(n))
        .map(|n| (n.node_id.clone(), task_types::s1::spec(n).ok().and_then(|s| s.kind).unwrap_or_else(|| "default".into())))
        .collect()
}

/// Load the trace of `rec`. Missing parts (an unreadable chain) load empty.
pub async fn load_trace(st: &AppState, s: &AgencyStore, rec: &RunRecord) -> anyhow::Result<Trace> {
    let run_id = rec.run["id"].as_str().unwrap_or_default();
    let events = s.events(run_id).await?;
    let receipts = st.rails.receipts.chain_store().and_then(|c| c.read_run(run_id)).unwrap_or_default();
    let journal = load_journal(&st.db, run_id)?;
    let decisions = load_decisions(&st.db, &rec.owner, run_id).unwrap_or_default();
    let divergences = recorded_walk(&st.db, rec, &events).await;
    let s1_kinds = s1_kinds(rec);
    Ok(Trace { run: rec.run.clone(), attention: rec.attention.clone(), events, receipts, journal, decisions, s1_kinds, divergences })
}

// ── the optional trace.error decision ───────────────────────────────────────

fn decision_backends() -> Option<Option<Vec<String>>> {
    let v = std::env::var(DECISION_ENV).ok()?;
    let v = v.trim();
    match v {
        "" | "0" | "off" | "false" => None,
        "1" | "on" | "true" => Some(None),
        list => Some(Some(list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())),
    }
}

/// The deterministic verdict: the most severe finding at high or above, else `none`.
pub fn label_of(findings: &[Finding]) -> &'static str {
    findings.iter().find(|f| f.severity >= Severity::High).map_or("none", |f| f.check)
}

/// The `trace.error` decision request: one option per error class plus `none`.
pub fn decision_request(run_id: &str, t: &Trace, findings: &[Finding], backends: Option<Vec<String>>) -> Value {
    let mut options = vec![json!({ "id": "none", "text": "No error in this run" })];
    options.extend(CHECKS.iter().map(|(id, title)| json!({ "id": id, "text": title })));
    let context = json!({ "run_status": t.run["status"], "task_type_signals": signals(t),
        "deterministic": findings.iter().map(|f| json!({ "check": f.check, "severity": f.severity.as_str() })).collect::<Vec<_>>() });
    let mut body = json!({ "kind": DECISION_KIND, "context": context.to_string(), "options": options,
        "question": "Does this agent run contain an error? Pick the error class, or none.",
        "allow_abstain": true, "latency_budget_ms": 5000, "task": format!("trace:{run_id}") });
    if let Some(b) = backends {
        body["backends"] = json!(b);
    }
    body
}

async fn decide(st: &Arc<AppState>, owner: &str, run_id: &str, t: &Trace, findings: &[Finding]) -> Value {
    let Some(backends) = decision_backends() else { return Value::Null };
    let user = AuthUser { user_id: owner.to_string(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None };
    let label = label_of(findings);
    match decisions::decide_value(st, &user, decision_request(run_id, t, findings, backends)).await {
        Ok(d) => {
            let id = d["id"].as_str().unwrap_or_default().to_string();
            let choice = d["choice"].as_str().map(String::from);
            // The deterministic verdict labels the decision (weak label): the flywheel learns from it.
            let status = if choice.as_deref() == Some(label) { "success" } else { "failure" };
            if let Err(e) = decisions::record_outcome(st, &user, &id, status, Some(label), Some("trace monitor deterministic verdict")) {
                tracing::warn!(run_id, error = %e, "trace.error outcome not recorded");
            }
            json!({ "decision_id": id, "choice": choice, "confidence": d["confidence"], "label": label, "agrees": status == "success" })
        }
        Err(e) => {
            tracing::warn!(run_id, error = %e, "trace.error decision failed");
            json!({ "error": e, "label": label })
        }
    }
}

// ── running ─────────────────────────────────────────────────────────────────

/// Check one terminal run, store its findings and log severe ones. Returns
/// the stored object (`None`: not terminal or not found).
pub async fn monitor_run(st: &Arc<AppState>, run_id: &str, with_decision: bool) -> anyhow::Result<Option<Value>> {
    let s = store(st);
    let Some(rec) = s.load_run(run_id).await? else { return Ok(None) };
    if !TERMINAL.contains(&rec.run["status"].as_str().unwrap_or_default()) {
        return Ok(None);
    }
    let t = load_trace(st, &s, &rec).await?;
    let findings = run_checks(&t);
    let decision = if with_decision { decide(st, &rec.owner, run_id, &t, &findings).await } else { Value::Null };
    let out = report(run_id, &t, &findings, decision);
    s.save_object(EV_FINDINGS, run_id, &rec.owner, &out).await?;
    for f in findings.iter().filter(|f| f.severity >= Severity::High) {
        let j = f.json(run_id);
        tracing::warn!(run_id, check = f.check, severity = f.severity.as_str(), "agency trace monitor finding");
        s.append_raw(EV_TRACE_FINDING, run_id, json!({ "run_id": run_id, "owner": rec.owner, "finding": j, "run_status": rec.run["status"] })).await?;
    }
    Ok(Some(out))
}

pub fn enabled() -> bool {
    std::env::var(ENABLE_ENV).map_or(true, |v| !matches!(v.trim(), "0" | "off" | "false"))
}

/// Hook the monitor onto this state's store: every terminal transition
/// spawns it, off path. Idempotent (once per ledger).
pub fn install(st: &Arc<AppState>) {
    let weak = Arc::downgrade(st);
    store(st).set_terminal_hook(move || -> super::store::TerminalHook {
        Arc::new(move |run_id: &str| {
            if !enabled() {
                return;
            }
            let (Some(st), Ok(h)) = (weak.upgrade(), tokio::runtime::Handle::try_current()) else { return };
            let id = run_id.to_string();
            h.spawn(async move {
                if let Err(e) = monitor_run(&st, &id, true).await {
                    tracing::warn!(run_id = %id, error = %e, "agency trace monitor failed (run unaffected)");
                }
            });
        })
    });
}

/// Stored findings of `run_id` for `owner`, if the monitor ran.
pub async fn stored(s: &AgencyStore, run_id: &str, owner: &str) -> anyhow::Result<Option<Value>> {
    s.load_object(EV_FINDINGS, run_id, owner).await
}

/// Severity counts of findings by check id (tests, the eval harness).
pub fn by_check(findings: &[Finding]) -> HashMap<&'static str, Severity> {
    findings.iter().map(|f| (f.check, f.severity)).collect()
}

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod tests;
