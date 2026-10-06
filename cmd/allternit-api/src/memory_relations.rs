//! Memory write path, decision M1 (WP-M1b): typed memory, typed relations,
//! and the S1 shadow on both.
//!
//! - [`MemoryType`] and [`RelationType`] are the closed sets V208 enforces.
//! - [`write_relation`] is the one edge writer: it writes the canonical typed
//!   edge into `memory_relationships` and mirrors it into `memory_edges` (the
//!   graph `/memory/edges` serves). Neither table had a writer before.
//! - [`shadow_turn`] asks S1 (`/v1/decision`, backend `auto` unless
//!   `ALLTERNIT_S1_BACKEND` says otherwise) for MEMORY_TYPE (LABEL motif) on
//!   the observation and RELATION (CHOICE) for each candidate memory, records
//!   each `x-decision_id`, and reports the incumbent's actual decision as the
//!   outcome label (`/v1/decision/outcome`). SHADOW ONLY: S1's answer never
//!   changes what is stored; the incumbent LLM op decides.
//! - [`report_user_label`] turns a user's edit or delete of a memory into an
//!   outcome for every S1 decision that typed or related it.
//!
//! Incumbent op → RELATION label (truth reported for the candidate memory):
//!
//! | incumbent op on candidate `c`          | label         | source                        |
//! |----------------------------------------|---------------|-------------------------------|
//! | `update {id: c}`                       | `updates`     | `memory.incumbent`            |
//! | `forget {id: c}`                       | `contradicts` | `memory.incumbent`            |
//! | `add {fact}` with the same text as `c` | `same`        | `memory.incumbent`            |
//! | nothing touched `c`                    | `unrelated`   | `memory.incumbent.untouched`  |
//!
//! The last row is a weak label (the incumbent has no `about_entity`/`causes`
//! vocabulary, so "untouched" can hide a real relation); it carries its own
//! source so calibration can filter or down-weight it.
//!
//! Incumbent → MEMORY_TYPE label for the observation: the `type` of the first
//! fact the turn wrote (`fact` when the model gave none); `not_memory` when the
//! model returned no operations; nothing (no label) when the turn only retired
//! memories or the model did not answer (the rule-based fallback is not a
//! decision about type).
//!
//! User labels: deleting a memory reports `not_memory` to its MEMORY_TYPE
//! decision (source `user.delete`); editing one reports the type it ends with
//! (source `user.edit`).

use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tracing::debug;
use uuid::Uuid;

use allternit_factory_engine::kernel::s1_outcome::OutcomeReporter;

use crate::db::DbHandle;
use crate::memory_kernel_service::MemoryKernelError;

pub const BANK_MEMORY_TYPE: &str = "memory.type";
pub const BANK_RELATION: &str = "memory.relation";
/// Most candidate memories judged by S1 RELATION per observation.
pub const MAX_RELATION_CANDIDATES: usize = 5;

macro_rules! closed_set {
    ($name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }
            /// Parse a label; accepts `-` for `_` and any case.
            pub fn parse(raw: &str) -> Option<Self> {
                let norm = raw.trim().to_ascii_lowercase().replace('-', "_");
                match norm.as_str() { $($s => Some($name::$var),)+ _ => None }
            }
        }
    };
}

closed_set!(MemoryType {
    Fact => "fact",
    Preference => "preference",
    Event => "event",
    Procedure => "procedure",
    Entity => "entity",
    Relationship => "relationship",
    TaskState => "task_state",
    NotMemory => "not_memory",
});

closed_set!(RelationType {
    Same => "same",
    Updates => "updates",
    Contradicts => "contradicts",
    Causes => "causes",
    CausedBy => "caused_by",
    PartOf => "part_of",
    AboutEntity => "about_entity",
    FollowsInTime => "follows_in_time",
    Unrelated => "unrelated",
});

impl RelationType {
    /// `updates` / `contradicts` end the target's validity (soft supersession).
    pub fn supersedes(self) -> bool {
        matches!(self, RelationType::Updates | RelationType::Contradicts)
    }
}

/// Endpoint kind of a typed edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Fact,
    Observation,
    Entity,
}
impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Fact => "fact",
            NodeKind::Observation => "observation",
            NodeKind::Entity => "entity",
        }
    }
}

/// Who decided an edge.
pub const ORIGIN_INCUMBENT: &str = "incumbent_llm";
pub const ORIGIN_USER: &str = "user";

/// Write one typed edge into `memory_relationships` (canonical) and mirror it
/// into `memory_edges`. `updates`/`contradicts` onto a fact also end that
/// fact's validity (`valid_until`) and drop its embedding. Returns the edge id.
#[allow(clippy::too_many_arguments)]
pub fn write_relation(
    conn: &Connection,
    user_id: &str,
    source: (NodeKind, &str),
    relation: RelationType,
    target: (NodeKind, &str),
    confidence: f64,
    origin: &str,
    decision_id: Option<&str>,
) -> rusqlite::Result<String> {
    let id = format!("rel_{}", Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO memory_relationships
           (id, user_id, source_entity_id, target_entity_id, relation, confidence,
            relation_type, source_kind, target_kind, origin, decision_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5, ?7, ?8, ?9, ?10)",
        params![id, user_id, source.1, target.1, relation.as_str(), confidence,
            source.0.as_str(), target.0.as_str(), origin, decision_id],
    )?;
    let metadata = json!({
        "relationship_id": id, "source_kind": source.0.as_str(), "target_kind": target.0.as_str(),
        "origin": origin, "decision_id": decision_id,
    });
    conn.execute(
        "INSERT INTO memory_edges (id, user_id, source, relationship, target, confidence, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![format!("edge_{}", Uuid::new_v4().simple()), user_id, source.1, relation.as_str(),
            target.1, confidence, metadata.to_string()],
    )?;
    if relation.supersedes() && target.0 == NodeKind::Fact {
        supersede_fact(conn, user_id, target.1)?;
    }
    Ok(id)
}

/// Soft supersession: end a fact's validity and drop it from vector recall.
/// Returns whether a still-valid fact was retired.
pub fn supersede_fact(conn: &Connection, user_id: &str, fact_id: &str) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE memory_facts SET valid_until = CURRENT_TIMESTAMP
         WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
        params![fact_id, user_id],
    )?;
    if changed > 0 {
        conn.execute(
            "DELETE FROM memory_embeddings WHERE user_id = ?1 AND target_type = 'fact' AND target_id = ?2",
            params![user_id, fact_id],
        )?;
    }
    Ok(changed > 0)
}

pub fn set_fact_type(conn: &Connection, user_id: &str, fact_id: &str, t: MemoryType) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE memory_facts SET memory_type = ?3 WHERE id = ?1 AND user_id = ?2",
        params![fact_id, user_id, t.as_str()],
    )?;
    Ok(())
}

pub fn set_observation_type(conn: &Connection, user_id: &str, obs_id: &str, t: MemoryType) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE memory_observations SET memory_type = ?3 WHERE id = ?1 AND user_id = ?2",
        params![obs_id, user_id, t.as_str()],
    )?;
    Ok(())
}

pub fn fact_type(conn: &Connection, user_id: &str, fact_id: &str) -> rusqlite::Result<Option<MemoryType>> {
    let raw: Option<Option<String>> = conn
        .query_row(
            "SELECT memory_type FROM memory_facts WHERE id = ?1 AND user_id = ?2",
            params![fact_id, user_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.flatten().as_deref().and_then(MemoryType::parse))
}

/// Typed edges touching a node (either end), newest first: (relation, source, target).
pub fn relations_of(conn: &Connection, user_id: &str, node_id: &str) -> rusqlite::Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT relation_type, source_entity_id, target_entity_id FROM memory_relationships
         WHERE user_id = ?1 AND relation_type IS NOT NULL AND (source_entity_id = ?2 OR target_entity_id = ?2)
         ORDER BY valid_from DESC, rowid DESC",
    )?;
    let rows = stmt.query_map(params![user_id, node_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

// ─── What the incumbent decided for one turn ────────────────────────────────

/// The incumbent's decisions for one observation, the labels the shadow
/// decisions are scored against.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnOutcome {
    /// MEMORY_TYPE label for the observation (None = no label, see module docs).
    pub observation_type: Option<MemoryType>,
    /// Facts written this turn and their type.
    pub produced: Vec<(String, MemoryType)>,
    /// Relations the incumbent asserted onto existing memories: (candidate fact id, relation, new fact id).
    pub relations: Vec<(String, RelationType, Option<String>)>,
}

impl TurnOutcome {
    /// Incumbent RELATION label for a candidate, and the outcome source.
    pub fn relation_label(&self, candidate_id: &str) -> (RelationType, &'static str, Option<String>) {
        match self.relations.iter().find(|(c, _, _)| c == candidate_id) {
            Some((_, r, produced)) => (*r, "memory.incumbent", produced.clone()),
            None => (RelationType::Unrelated, "memory.incumbent.untouched", None),
        }
    }
}

// ─── S1 shadow ──────────────────────────────────────────────────────────────

/// Client for the decision runtime (`/v1/decision`). Same URL/token env as
/// [`OutcomeReporter`]; injectable for tests.
#[derive(Debug, Clone)]
pub struct S1Client {
    pub reporter: OutcomeReporter,
    pub backend: String,
    pub enabled: bool,
}

/// One S1 answer and its ledger join key.
#[derive(Debug, Clone, PartialEq)]
pub struct S1Answer {
    pub decision_id: String,
    pub answer: String,
    pub confidence: f64,
}

impl S1Client {
    /// `ALLTERNIT_S1_BACKEND` (default `auto`; `off` disables) and
    /// `ALLTERNIT_S1_MEMORY_SHADOW=0` to turn the memory shadow off. Shadow is
    /// on by default (Q28).
    pub fn from_env() -> Self {
        let backend = std::env::var("ALLTERNIT_S1_BACKEND").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "auto".into());
        let reporter = OutcomeReporter::from_env();
        let enabled = backend != "off"
            && reporter.enabled
            && std::env::var("ALLTERNIT_S1_MEMORY_SHADOW").map(|v| v != "0").unwrap_or(true);
        Self { reporter, backend, enabled }
    }

    pub fn new(base_url: &str) -> Self {
        Self {
            reporter: OutcomeReporter { base_url: base_url.trim_end_matches('/').into(), token: None, timeout: Duration::from_millis(1500), enabled: true },
            backend: "auto".into(),
            enabled: true,
        }
    }

    /// One closed-set decision. Never errors: an unreachable or failing
    /// runtime is `None`.
    pub async fn decide(&self, bank: &str, motif: &str, node: &str, obs_id: &str, instructions: &str, state: &str, labels: &[&str]) -> Option<S1Answer> {
        if !self.enabled {
            return None;
        }
        let body = decision_body(&self.backend, bank, motif, node, obs_id, instructions, state, labels);
        let c = reqwest::Client::builder().timeout(self.reporter.timeout).build().ok()?;
        let mut rq = c.post(format!("{}/v1/decision", self.reporter.base_url)).json(&body);
        if let Some(t) = &self.reporter.token {
            rq = rq.bearer_auth(t);
        }
        let r = rq.send().await.ok()?;
        if !r.status().is_success() {
            debug!(status = %r.status(), bank, "s1 memory shadow decision rejected");
            return None;
        }
        let v: Value = r.json().await.ok()?;
        let decision_id = v["extensions"]["x-decision_id"].as_str()?.to_string();
        let answer = match &v["answer"] {
            Value::String(s) => s.clone(),
            a => a["candidate_id"].as_str().or(a["label"].as_str()).unwrap_or_default().to_string(),
        };
        Some(S1Answer { decision_id, answer, confidence: v["confidence"].as_f64().unwrap_or(0.0) })
    }
}

/// `/v1/decision` body for a CHOICE over a closed label set (+ the unknown
/// candidate the LABEL motif requires).
#[allow(clippy::too_many_arguments)]
pub fn decision_body(backend: &str, bank: &str, motif: &str, node: &str, obs_id: &str, instructions: &str, state: &str, labels: &[&str]) -> Value {
    let candidates: Vec<Value> = labels
        .iter()
        .map(|l| json!({ "candidate_id": l, "label": l.replace('_', " ") }))
        .chain(std::iter::once(json!({ "candidate_id": "unknown", "label": "unknown", "is_unknown": true })))
        .collect();
    json!({ "state": state, "reversible": true, "backend": backend, "request": {
        "envelope": { "abi_version": "1.0.0", "schema_id": "allternit.kernel.DecisionRequestV1", "schema_version": "1.0.0",
            "run_id": format!("memory:{obs_id}"), "node_id": node },
        "operation": "CHOICE", "state_projection_ref": format!("memory:{obs_id}:{node}"),
        "instructions": instructions, "decision_bank_id": bank, "candidates": candidates,
        "calibration_domain": bank, "latency_class": "BATCH",
        "extensions": { "x-motif": motif } } })
}

pub(crate) fn record_decision(
    conn: &Connection,
    user_id: &str,
    bank: &str,
    ans: &S1Answer,
    obs_id: &str,
    candidate: Option<&str>,
    produced: Option<&str>,
    incumbent: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO memory_s1_decisions
           (id, user_id, bank, decision_id, observation_id, candidate_fact_id, produced_fact_id, s1_answer, incumbent_label)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![format!("msd_{}", Uuid::new_v4().simple()), user_id, bank, ans.decision_id, obs_id,
            candidate, produced, ans.answer, incumbent],
    )?;
    Ok(())
}

const TYPE_INSTRUCTIONS: &str = "Classify what kind of long-term memory about the user this message carries. \
not_memory = nothing durable (questions, requests, one-off context, world facts).";
const RELATION_INSTRUCTIONS: &str = "How does the new message relate to the existing memory? \
updates = it replaces the memory with a newer value; contradicts = it says the memory is no longer true; \
same = it restates it; unrelated = no relation.";

/// What the shadow did, for logs/tests.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ShadowReport {
    pub decisions: usize,
    pub outcomes_reported: usize,
}

/// The S1 shadow for one observation: MEMORY_TYPE on the message, RELATION
/// for each candidate memory (id, text), then the incumbent's labels as
/// outcomes. Never changes stored memory except the decision ledger and the
/// `decision_id` on the matching incumbent edge.
pub async fn shadow_turn(
    client: &S1Client,
    db: &DbHandle,
    user_id: &str,
    obs_id: &str,
    message: &str,
    candidates: &[(String, String)],
    outcome: &TurnOutcome,
) -> ShadowReport {
    let mut report = ShadowReport::default();
    if !client.enabled {
        return report;
    }
    let type_labels: Vec<&str> = MemoryType::ALL.iter().map(|t| t.as_str()).collect();
    let rel_labels: Vec<&str> = RelationType::ALL.iter().map(|t| t.as_str()).collect();
    let msg: String = message.chars().take(2000).collect();

    // MEMORY_TYPE (LABEL motif).
    if let Some(ans) = client.decide(BANK_MEMORY_TYPE, "LABEL", "memory.type", obs_id, TYPE_INSTRUCTIONS, &msg, &type_labels).await {
        report.decisions += 1;
        let incumbent = outcome.observation_type.map(|t| t.as_str());
        if let Ok(conn) = db.connect() {
            if outcome.produced.is_empty() {
                let _ = record_decision(&conn, user_id, BANK_MEMORY_TYPE, &ans, obs_id, None, None, incumbent);
            }
            for (fact_id, _) in &outcome.produced {
                let _ = record_decision(&conn, user_id, BANK_MEMORY_TYPE, &ans, obs_id, None, Some(fact_id), incumbent);
            }
        }
        if let Some(label) = incumbent {
            if client.reporter.report(&ans.decision_id, label, "memory.incumbent").await {
                report.outcomes_reported += 1;
            }
        }
    }

    // RELATION (CHOICE) per candidate memory.
    for (cand_id, cand_text) in candidates.iter().take(MAX_RELATION_CANDIDATES) {
        let state = format!("new message:\n{msg}\n\nexisting memory:\n{cand_text}");
        let Some(ans) = client.decide(BANK_RELATION, "CHOICE", "memory.relation", obs_id, RELATION_INSTRUCTIONS, &state, &rel_labels).await else {
            continue;
        };
        report.decisions += 1;
        let (label, source, produced) = outcome.relation_label(cand_id);
        if let Ok(conn) = db.connect() {
            let _ = record_decision(&conn, user_id, BANK_RELATION, &ans, obs_id, Some(cand_id), produced.as_deref(), Some(label.as_str()));
            // Join key on the incumbent's edge for this candidate.
            let _ = conn.execute(
                "UPDATE memory_relationships SET decision_id = ?3
                 WHERE user_id = ?1 AND target_entity_id = ?2 AND decision_id IS NULL AND origin = 'incumbent_llm'
                   AND source_entity_id IN (?4, ?5)",
                params![user_id, cand_id, ans.decision_id, obs_id, produced.as_deref().unwrap_or(obs_id)],
            );
        }
        if client.reporter.report(&ans.decision_id, label.as_str(), source).await {
            report.outcomes_reported += 1;
        }
    }
    report
}

/// A user's edit of one of their memories. New text writes a new fact that
/// `updates` the old one (origin `user`, so the old one is soft-superseded);
/// a type-only edit retypes in place. Returns the current fact id and its
/// type, or None when the fact is not the user's or no longer current.
pub fn edit_fact(
    db: &DbHandle,
    user_id: &str,
    fact_id: &str,
    new_text: Option<&str>,
    new_type: Option<MemoryType>,
) -> Result<Option<(String, MemoryType)>, MemoryKernelError> {
    let conn = db.connect()?;
    let row: Option<(String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT fact, agent_id, source_observation_id, memory_type FROM memory_facts
             WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
            params![fact_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((old_text, agent_id, obs_id, old_type)) = row else { return Ok(None) };
    let t = new_type.or_else(|| old_type.as_deref().and_then(MemoryType::parse)).unwrap_or(MemoryType::Fact);
    let text = new_text.map(str::trim).filter(|t| !t.is_empty() && *t != old_text);
    let Some(text) = text else {
        set_fact_type(&conn, user_id, fact_id, t)?;
        return Ok(Some((fact_id.to_string(), t)));
    };
    // Retire the old row first so the dedup in persist_facts does not see it.
    supersede_fact(&conn, user_id, fact_id)?;
    let obs = obs_id.unwrap_or_default();
    let new = crate::memory_kernel_service::persist_facts(db, user_id, agent_id.as_deref(), &obs, &[text.to_string()])?;
    let new_id = match new.into_iter().next() {
        Some(f) => f.id,
        // Same text as another current memory (or a secret): keep that one.
        None => conn
            .query_row(
                "SELECT id FROM memory_facts WHERE user_id = ?1 AND lower(fact) = lower(?2) AND valid_until IS NULL LIMIT 1",
                params![user_id, text],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default(),
    };
    if new_id.is_empty() {
        return Ok(None);
    }
    set_fact_type(&conn, user_id, &new_id, t)?;
    write_relation(&conn, user_id, (NodeKind::Fact, &new_id), RelationType::Updates, (NodeKind::Fact, fact_id), 1.0, ORIGIN_USER, None)?;
    Ok(Some((new_id, t)))
}

/// A user edited or deleted a memory: report the truth for every S1
/// MEMORY_TYPE decision that typed it. Returns the decision ids reported.
pub fn user_label_decisions(
    conn: &Connection,
    user_id: &str,
    fact_id: &str,
    label: MemoryType,
) -> Result<Vec<String>, MemoryKernelError> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT decision_id FROM memory_s1_decisions
             WHERE user_id = ?1 AND produced_fact_id = ?2 AND bank = ?3",
        )?;
        let rows = stmt.query_map(params![user_id, fact_id, BANK_MEMORY_TYPE], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    conn.execute(
        "UPDATE memory_s1_decisions SET user_label = ?3 WHERE user_id = ?1 AND produced_fact_id = ?2 AND bank = ?4",
        params![user_id, fact_id, label.as_str(), BANK_MEMORY_TYPE],
    )?;
    Ok(ids)
}

/// [`user_label_decisions`] plus the detached outcome POSTs (needs a tokio runtime).
pub fn report_user_label(db: &DbHandle, user_id: &str, fact_id: &str, label: MemoryType, source: &str) {
    let Ok(conn) = db.connect() else { return };
    let Ok(ids) = user_label_decisions(&conn, user_id, fact_id, label) else { return };
    let reporter = OutcomeReporter::from_env();
    for id in ids {
        reporter.spawn_report(id, label.as_str().to_string(), source.to_string());
    }
}

#[cfg(test)]
mod tests;
