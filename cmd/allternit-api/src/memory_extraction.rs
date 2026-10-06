//! Model-based memory extraction with reconciliation.
//!
//! The pattern used by production assistant memory (mem0, Zep, ChatGPT and
//! Claude memory): after a user turn, a small model reads the message next to
//! the user's current memories and returns operations — add a new durable
//! fact, update one that changed, forget one the user retracted, or nothing.
//! Updates and forgets are soft: the old fact gets `valid_until`, so history
//! stays auditable while recall only sees what is currently true.
//!
//! The model is reached through gizzi-code (`gizzi_completion`), never a
//! provider directly. When no model answers, the rule-based extractor in
//! `memory_kernel_service` runs instead, so memory never depends on a model
//! being configured.
//!
//! M1 write path (WP-M1b): each add/update carries a memory type, the insert
//! writes typed edges (`memory_relations`), and S1 runs in shadow on the
//! closed-set parts (MEMORY_TYPE, RELATION) with this model's op as the
//! incumbent that decides. The reply is schema-constrained (O10): gizzi's
//! json_schema format where the lane honors it, and [`validate_reply`]
//! always.

use rusqlite::params;
use serde::Deserialize;
use std::sync::OnceLock;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::db::DbHandle;
use crate::memory_kernel_service::{self as kernel, MemoryKernelError};
use crate::memory_relations::{self as rel, MemoryType, NodeKind, RelationType, TurnOutcome};

/// Existing memories shown to the model: all of them when there are few,
/// otherwise the most similar to the message.
const ALL_FACTS_UP_TO: usize = 40;
const SIMILAR_FACTS: usize = 25;
/// Most facts one turn may add; guards against a model dumping the message.
const MAX_ADDS_PER_TURN: usize = 5;
const MAX_FACT_CHARS: usize = 240;

const SYSTEM_PROMPT: &str = "You maintain a long-term memory about the user of an AI assistant. \
You only output JSON.";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum MemoryOp {
    Add {
        fact: String,
        #[serde(default, rename = "type")]
        memory_type: Option<String>,
    },
    Update {
        id: String,
        fact: String,
        #[serde(default, rename = "type")]
        memory_type: Option<String>,
    },
    Forget { id: String },
}

impl MemoryOp {
    /// The op's memory type; `fact` when missing or outside the closed set.
    fn typed(raw: &Option<String>) -> MemoryType {
        raw.as_deref().and_then(MemoryType::parse).filter(|t| *t != MemoryType::NotMemory).unwrap_or(MemoryType::Fact)
    }
}

/// The `gen.small` lane (O1): fact text is easy generation, so extraction
/// defaults to a small model. Harness adapter default; the capability-class
/// router (WP-R1) will own this once it is on the path.
const DEFAULT_SMALL_MODEL: (&str, &str) = ("claude-cli", "claude-haiku-4-5");

fn parse_model(raw: &str) -> Option<(String, String)> {
    raw.trim().split_once('/').filter(|(p, m)| !p.is_empty() && !m.is_empty()).map(|(p, m)| (p.to_string(), m.to_string()))
}

/// The model used for extraction: `ALLTERNIT_MEMORY_MODEL` ("provider/model"),
/// else the shared small lane `ALLTERNIT_GEN_SMALL_MODEL`, else the built-in
/// small default.
pub fn extraction_model() -> (String, String) {
    ["ALLTERNIT_MEMORY_MODEL", "ALLTERNIT_GEN_SMALL_MODEL"]
        .iter()
        .find_map(|k| std::env::var(k).ok().as_deref().and_then(parse_model))
        .unwrap_or_else(|| (DEFAULT_SMALL_MODEL.0.to_string(), DEFAULT_SMALL_MODEL.1.to_string()))
}

/// JSON schema of the extraction reply (O10). Sent as gizzi's json_schema
/// format and enforced locally by [`validate_reply`].
pub fn reply_schema() -> serde_json::Value {
    let types: Vec<&str> = MemoryType::ALL.iter().map(|t| t.as_str()).filter(|t| *t != "not_memory").collect();
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["operations"],
        "properties": { "operations": { "type": "array", "items": { "type": "object",
            "required": ["op"],
            "properties": {
                "op": { "type": "string", "enum": ["add", "update", "forget"] },
                "id": { "type": "string" },
                "fact": { "type": "string", "maxLength": MAX_FACT_CHARS },
                "type": { "type": "string", "enum": types }
            } } } }
    })
}

/// Schema-check one parsed reply: `operations` must be an array; each op must
/// carry the fields its kind requires (`add`: fact; `update`: id + fact;
/// `forget`: id) as strings, and `type`, when present, must be in the closed
/// set. Ops that fail are dropped; a reply without an `operations` array is
/// rejected (the rule-based fallback runs instead).
pub fn validate_reply(value: &serde_json::Value) -> Option<Vec<MemoryOp>> {
    let ops = value.get("operations")?.as_array()?;
    let s = |op: &serde_json::Value, k: &str| op.get(k).map(|v| v.is_string()).unwrap_or(false);
    Some(
        ops.iter()
            .filter(|op| {
                let type_ok = match op.get("type") {
                    None | Some(serde_json::Value::Null) => true,
                    Some(t) => t.as_str().and_then(MemoryType::parse).is_some(),
                };
                let fields_ok = match op.get("op").and_then(|v| v.as_str()) {
                    Some("add") => s(op, "fact"),
                    Some("update") => s(op, "id") && s(op, "fact"),
                    Some("forget") => s(op, "id"),
                    _ => false,
                };
                type_ok && fields_ok
            })
            .filter_map(|op| serde_json::from_value::<MemoryOp>(op.clone()).ok())
            .collect(),
    )
}

/// Build the extraction prompt for one user message.
pub fn build_prompt(message: &str, existing: &[(String, String)]) -> String {
    let memories = if existing.is_empty() {
        "(none yet)".to_string()
    } else {
        existing
            .iter()
            .map(|(id, fact)| format!("- [{id}] {fact}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"Decide what, if anything, to remember about the user from their latest message.

Remember only durable facts about the user that will still matter in future conversations:
identity, role, work, projects, tools and stack, preferences, how they like answers,
relationships, recurring constraints, and things they explicitly ask you to remember.

Do NOT remember:
- questions, requests, or tasks ("what is 17 times 23", "write me an email")
- facts about the world, or anything the assistant said
- one-off or temporary context ("I'm tired today", "this chat is a test")
- secrets: passwords, API keys, card or account numbers
- sensitive details (health, religion, politics, sexuality) unless the user explicitly asks you to remember them

Write each fact as one short, self-contained sentence in the third person
("User is a product designer in Austin.", "User prefers TypeScript over JavaScript.").
Give each add/update a "type": fact, preference, event, procedure, entity, relationship or task_state.

Current memories:
{memories}

Latest user message:
"""
{message}
"""

Return JSON only, in this shape:
{{"operations": [
  {{"op": "add", "fact": "...", "type": "preference"}},
  {{"op": "update", "id": "<id of the memory it replaces>", "fact": "...", "type": "fact"}},
  {{"op": "forget", "id": "<id the user retracted or said is no longer true>"}}
]}}
Use update when the message changes an existing memory, not add. Never add a fact that is
already in current memories. If nothing is worth remembering, return {{"operations": []}}."#
    )
}

/// Parse the model's reply. Tolerates prose or code fences around the JSON.
pub fn parse_ops(raw: &str) -> Option<Vec<MemoryOp>> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    let slice = &raw[start..=end];
    // Unknown or malformed ops are dropped instead of failing the whole reply.
    let value: serde_json::Value = serde_json::from_str(slice).ok()?;
    validate_reply(&value)
}

/// A fact the model returned that is fit to store.
fn clean_fact(fact: &str) -> Option<String> {
    let fact = fact.trim();
    let len = fact.chars().count();
    if !(6..=MAX_FACT_CHARS).contains(&len) || fact.ends_with('?') {
        return None;
    }
    (!kernel::mentions_secret(fact)).then(|| fact.to_string())
}

/// Current memories to show the model for this message.
pub fn candidate_facts(
    db: &DbHandle,
    user_id: &str,
    message: &str,
) -> Result<Vec<(String, String)>, MemoryKernelError> {
    let all = kernel::list_facts(db, user_id, None, ALL_FACTS_UP_TO + 1)?;
    if all.len() <= ALL_FACTS_UP_TO {
        return Ok(all.into_iter().map(|f| (f.id, f.fact)).collect());
    }
    let conn = db.connect()?;
    let mut out = Vec::new();
    for (target_type, id, _) in kernel::recall_semantic(db, user_id, message, SIMILAR_FACTS * 2)? {
        if target_type != "fact" || out.len() >= SIMILAR_FACTS {
            continue;
        }
        let fact: Option<String> = conn
            .query_row(
                "SELECT fact FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
                params![id, user_id],
                |row| row.get(0),
            )
            .ok();
        if let Some(fact) = fact {
            out.push((id, fact));
        }
    }
    Ok(out)
}

/// Only facts shown to the model may be touched, so a hallucinated id cannot
/// erase an unrelated memory.
fn shown_and_valid(conn: &rusqlite::Connection, user_id: &str, id: &str, shown: &[(String, String)]) -> Result<bool, MemoryKernelError> {
    if !shown.iter().any(|(sid, _)| sid == id) {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL)",
        params![id, user_id],
        |r| r.get(0),
    )?)
}

/// Active fact with this exact text (case-insensitive), if any.
fn existing_fact_id(conn: &rusqlite::Connection, user_id: &str, fact: &str) -> Result<Option<String>, MemoryKernelError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT id FROM memory_facts WHERE user_id = ?1 AND lower(fact) = lower(?2) AND valid_until IS NULL LIMIT 1",
            params![user_id, fact],
            |r| r.get(0),
        )
        .optional()?)
}

/// Apply model operations. Returns how many memories changed.
pub fn apply_ops(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    observation_id: &str,
    ops: &[MemoryOp],
    shown: &[(String, String)],
) -> Result<usize, MemoryKernelError> {
    apply_ops_typed(db, user_id, agent_id, observation_id, ops, shown).map(|(n, _)| n)
}

/// Apply model operations and write the M1 graph from them: memory types on
/// new facts and the observation, typed edges (`update` → new fact
/// `updates` old; `forget` → observation `contradicts` fact; an `add` that
/// restates a current memory → observation `same` fact), with soft
/// supersession through `valid_until`. Returns the change count and the
/// incumbent's decisions (the S1 shadow's labels).
pub fn apply_ops_typed(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    observation_id: &str,
    ops: &[MemoryOp],
    shown: &[(String, String)],
) -> Result<(usize, TurnOutcome), MemoryKernelError> {
    let conn = db.connect()?;
    if crate::memory_drive_writer::configured_root(&conn).ok().flatten().is_some() {
        return apply_ops_drive(db, &conn, user_id, agent_id, observation_id, ops, shown);
    }
    let mut changed = 0;
    let mut outcome = TurnOutcome::default();
    // (fact text, type, fact id it updates)
    let mut writes: Vec<(String, MemoryType, Option<String>)> = Vec::new();
    let mut adds = 0;
    for op in ops {
        match op {
            MemoryOp::Add { fact, memory_type } => {
                let Some(fact) = clean_fact(fact) else { continue };
                if let Some(existing) = existing_fact_id(&conn, user_id, &fact)? {
                    rel::write_relation(&conn, user_id, (NodeKind::Observation, observation_id), RelationType::Same,
                        (NodeKind::Fact, &existing), 0.85, rel::ORIGIN_INCUMBENT, None)?;
                    outcome.relations.push((existing, RelationType::Same, None));
                } else if adds < MAX_ADDS_PER_TURN {
                    adds += 1;
                    writes.push((fact, MemoryOp::typed(memory_type), None));
                }
            }
            MemoryOp::Update { id, fact, memory_type } => {
                if let Some(fact) = clean_fact(fact) {
                    if shown_and_valid(&conn, user_id, id, shown)? {
                        writes.push((fact, MemoryOp::typed(memory_type), Some(id.clone())));
                    }
                }
            }
            MemoryOp::Forget { id } => {
                if shown_and_valid(&conn, user_id, id, shown)? {
                    rel::write_relation(&conn, user_id, (NodeKind::Observation, observation_id), RelationType::Contradicts,
                        (NodeKind::Fact, id), 0.85, rel::ORIGIN_INCUMBENT, None)?;
                    outcome.relations.push((id.clone(), RelationType::Contradicts, None));
                    changed += 1;
                }
            }
        }
    }
    for (fact, t, updates) in writes {
        // An update to a restated value still retires the old memory.
        let new_id = match kernel::persist_facts(db, user_id, agent_id, observation_id, std::slice::from_ref(&fact))?.pop() {
            Some(f) => {
                changed += 1;
                rel::set_fact_type(&conn, user_id, &f.id, t)?;
                outcome.produced.push((f.id.clone(), t));
                Some(f.id)
            }
            None => existing_fact_id(&conn, user_id, &fact)?,
        };
        if let Some(old) = updates {
            match new_id.as_deref().filter(|n| *n != old) {
                Some(n) => {
                    rel::write_relation(&conn, user_id, (NodeKind::Fact, n), RelationType::Updates,
                        (NodeKind::Fact, &old), 0.85, rel::ORIGIN_INCUMBENT, None)?;
                }
                None => {
                    rel::supersede_fact(&conn, user_id, &old)?;
                }
            }
            changed += 1;
            outcome.relations.push((old, RelationType::Updates, new_id.clone()));
        }
    }
    outcome.observation_type = match outcome.produced.first() {
        Some((_, t)) => Some(*t),
        None if ops.is_empty() => Some(MemoryType::NotMemory),
        None => None,
    };
    if let Some(t) = outcome.observation_type {
        rel::set_observation_type(&conn, user_id, observation_id, t)?;
    }
    Ok((changed, outcome))
}

/// Memory Drive path: every add, update and forget of one turn lands as ONE
/// drive commit; graph edges are written afterwards as index metadata.
fn apply_ops_drive(
    db: &DbHandle,
    conn: &rusqlite::Connection,
    user_id: &str,
    agent_id: Option<&str>,
    observation_id: &str,
    ops: &[MemoryOp],
    shown: &[(String, String)],
) -> Result<(usize, TurnOutcome), MemoryKernelError> {
    let mut changed = 0;
    let mut outcome = TurnOutcome::default();
    let mut writes: Vec<(String, MemoryType, Option<String>)> = Vec::new();
    let mut forgets: Vec<String> = Vec::new();
    let mut adds = 0;
    for op in ops {
        match op {
            MemoryOp::Add { fact, memory_type } => {
                let Some(fact) = clean_fact(fact) else { continue };
                if let Some(existing) = existing_fact_id(conn, user_id, &fact)? {
                    rel::write_relation(conn, user_id, (NodeKind::Observation, observation_id), RelationType::Same,
                        (NodeKind::Fact, &existing), 0.85, rel::ORIGIN_INCUMBENT, None)?;
                    outcome.relations.push((existing, RelationType::Same, None));
                } else if adds < MAX_ADDS_PER_TURN {
                    adds += 1;
                    writes.push((fact, MemoryOp::typed(memory_type), None));
                }
            }
            MemoryOp::Update { id, fact, memory_type } => {
                if let Some(fact) = clean_fact(fact) {
                    if shown_and_valid(conn, user_id, id, shown)? {
                        writes.push((fact, MemoryOp::typed(memory_type), Some(id.clone())));
                    }
                }
            }
            MemoryOp::Forget { id } => {
                if shown_and_valid(conn, user_id, id, shown)? && !forgets.contains(id) {
                    forgets.push(id.clone());
                }
            }
        }
    }
    let mut retire = forgets.clone();
    retire.extend(writes.iter().filter_map(|(_, _, old)| old.clone()));
    let new_facts: Vec<crate::memory_drive_writer::NewFact> = writes
        .iter()
        .map(|(fact, t, _)| crate::memory_drive_writer::NewFact {
            text: fact.clone(),
            memory_type: Some(t.as_str().to_string()),
            agent: agent_id.map(str::to_string),
            observation: Some(observation_id.to_string()),
            ..Default::default()
        })
        .collect();
    let out = crate::memory_drive_writer::commit_facts(db, user_id, &new_facts, &retire, "Remember from conversation")
        .map_err(|e| MemoryKernelError::Internal(e.to_string()))?
        .ok_or_else(|| MemoryKernelError::Internal("memory drive is not configured".into()))?;
    for legacy in &out.legacy_retire {
        rel::supersede_fact(conn, user_id, legacy)?;
    }
    for ((fact, t, updates), id) in writes.into_iter().zip(out.fact_ids) {
        let new_id = match id {
            Some(id) => {
                changed += 1;
                outcome.produced.push((id.clone(), t));
                Some(id)
            }
            None => existing_fact_id(conn, user_id, &fact)?,
        };
        if let Some(old) = updates {
            if let Some(n) = new_id.as_deref().filter(|n| *n != old) {
                rel::write_relation(conn, user_id, (NodeKind::Fact, n), RelationType::Updates,
                    (NodeKind::Fact, &old), 0.85, rel::ORIGIN_INCUMBENT, None)?;
            }
            changed += 1;
            outcome.relations.push((old, RelationType::Updates, new_id.clone()));
        }
    }
    for id in forgets {
        rel::write_relation(conn, user_id, (NodeKind::Observation, observation_id), RelationType::Contradicts,
            (NodeKind::Fact, &id), 0.85, rel::ORIGIN_INCUMBENT, None)?;
        outcome.relations.push((id, RelationType::Contradicts, None));
        changed += 1;
    }
    outcome.observation_type = match outcome.produced.first() {
        Some((_, t)) => Some(*t),
        None if ops.is_empty() => Some(MemoryType::NotMemory),
        None => None,
    };
    if let Some(t) = outcome.observation_type {
        rel::set_observation_type(conn, user_id, observation_id, t)?;
    }
    Ok((changed, outcome))
}

/// Candidate memories S1 RELATION judges: those the incumbent touched first,
/// then the closest current facts from the hybrid index.
pub fn relation_candidates(
    db: &DbHandle,
    user_id: &str,
    message: &str,
    ops: &[MemoryOp],
    shown: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for op in ops {
        let id = match op {
            MemoryOp::Update { id, .. } | MemoryOp::Forget { id } => id,
            MemoryOp::Add { .. } => continue,
        };
        if let Some(c) = shown.iter().find(|(sid, _)| sid == id) {
            if !out.iter().any(|(o, _)| o == id) {
                out.push(c.clone());
            }
        }
    }
    let Ok(conn) = db.connect() else { return out };
    if let Ok(hits) = kernel::recall_semantic(db, user_id, message, rel::MAX_RELATION_CANDIDATES * 2) {
        for (target_type, id, _) in hits {
            if out.len() >= rel::MAX_RELATION_CANDIDATES {
                break;
            }
            if target_type != "fact" || out.iter().any(|(o, _)| *o == id) {
                continue;
            }
            let fact: Option<String> = conn
                .query_row(
                    "SELECT fact FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
                    params![id, user_id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(fact) = fact {
                out.push((id, fact));
            }
        }
    }
    out.truncate(rel::MAX_RELATION_CANDIDATES);
    out
}

/// At most two extractions in flight; extra turns wait instead of piling
/// temporary sessions onto gizzi-code.
fn extraction_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(2))
}

/// Extract and reconcile memories for one user message. Falls back to the
/// rule-based extractor when no model answers or the reply is unusable.
pub async fn extract_and_reconcile(
    db: DbHandle,
    user_id: String,
    agent_id: Option<String>,
    observation_id: String,
    message: String,
) {
    let _slot = extraction_slots().acquire().await;
    let shown = {
        let (db, user_id, message) = (db.clone(), user_id.clone(), message.clone());
        tokio::task::spawn_blocking(move || candidate_facts(&db, &user_id, &message))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    };

    let model = extraction_model();
    let reply = crate::usage_ledger::scope(
        crate::usage_ledger::LedgerCtx::surface("memory").tenant(None, Some(&user_id)),
        crate::gizzi_completion::complete_ephemeral_structured(
            &build_prompt(&message, &shown),
            Some(SYSTEM_PROMPT),
            Some(&model),
            &reply_schema(),
        ),
    )
    .await;
    let ops = reply.as_deref().and_then(parse_ops);

    // Candidates are read before the ops apply (an update retires its target).
    let candidates = match &ops {
        Some(ops) => {
            let (db, user_id, message, ops, shown) = (db.clone(), user_id.clone(), message.clone(), ops.clone(), shown.clone());
            tokio::task::spawn_blocking(move || relation_candidates(&db, &user_id, &message, &ops, &shown))
                .await
                .unwrap_or_default()
        }
        None => vec![],
    };

    let result = {
        let (db, user_id, observation_id, message, ops) = (db.clone(), user_id.clone(), observation_id.clone(), message.clone(), ops.clone());
        tokio::task::spawn_blocking(move || {
            let agent = agent_id.as_deref();
            match ops {
                Some(ops) => apply_ops_typed(&db, &user_id, agent, &observation_id, &ops, &shown).map(|(n, o)| (n, Some(o))),
                None => {
                    let facts = kernel::extract_facts_heuristic(&message);
                    if facts.is_empty() {
                        Ok((0, None))
                    } else {
                        kernel::persist_facts(&db, &user_id, agent, &observation_id, &facts).map(|p| (p.len(), None))
                    }
                }
            }
        })
        .await
    };

    match result {
        Ok(Ok((n, outcome))) => {
            debug!(changed = n, "memory extraction applied");
            // S1 shadow: only when the incumbent model actually decided.
            if let Some(outcome) = outcome {
                let client = rel::S1Client::from_env();
                let r = rel::shadow_turn(&client, &db, &user_id, &observation_id, &message, &candidates, &outcome).await;
                debug!(decisions = r.decisions, outcomes = r.outcomes_reported, "memory s1 shadow");
            }
        }
        Ok(Err(e)) => warn!("memory extraction failed: {e}"),
        Err(e) => warn!("memory extraction task failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> DbHandle {
        DbHandle::new_memory().expect("memory db")
    }

    #[test]
    fn parses_ops_inside_code_fences() {
        let raw = "Here you go:\n```json\n{\"operations\": [{\"op\": \"add\", \"fact\": \"User works in Rust.\"}, {\"op\": \"rename\", \"id\": \"x\"}, {\"op\": \"forget\", \"id\": \"fact_1\"}]}\n```";
        assert_eq!(
            parse_ops(raw).unwrap(),
            vec![
                MemoryOp::Add { fact: "User works in Rust.".into(), memory_type: None },
                MemoryOp::Forget { id: "fact_1".into() },
            ]
        );
        assert_eq!(parse_ops("{\"operations\": []}").unwrap(), vec![]);
        assert!(parse_ops("no json here").is_none());
    }

    #[test]
    fn schema_validator_drops_malformed_ops_and_rejects_bad_replies() {
        let v = serde_json::json!({"operations": [
            {"op": "add", "fact": "User likes Go.", "type": "preference"},
            {"op": "add", "fact": "User likes Zig.", "type": "opinion"},
            {"op": "update", "fact": "missing id"},
            {"op": "forget", "id": 7},
            {"op": "forget", "id": "fact_1"},
            {"op": "add"}
        ]});
        assert_eq!(
            validate_reply(&v).unwrap(),
            vec![
                MemoryOp::Add { fact: "User likes Go.".into(), memory_type: Some("preference".into()) },
                MemoryOp::Forget { id: "fact_1".into() },
            ]
        );
        assert!(validate_reply(&serde_json::json!({"ops": []})).is_none());
        assert!(validate_reply(&serde_json::json!({"operations": {}})).is_none());
        // The schema sent to the model names exactly the closed type set (minus not_memory).
        let schema = reply_schema();
        let types = schema["properties"]["operations"]["items"]["properties"]["type"]["enum"].as_array().unwrap();
        assert_eq!(types.len(), 7);
        assert!(!types.iter().any(|t| t == "not_memory"));
    }

    #[test]
    fn extraction_defaults_to_the_small_lane() {
        // Only checks the parse + default shape; env is process-global so the
        // override paths are covered by parse_model.
        assert_eq!(parse_model("prov/small-1"), Some(("prov".into(), "small-1".into())));
        assert_eq!(parse_model("noslash"), None);
        assert_eq!(parse_model("/m"), None);
        if std::env::var("ALLTERNIT_MEMORY_MODEL").is_err() && std::env::var("ALLTERNIT_GEN_SMALL_MODEL").is_err() {
            assert_eq!(extraction_model(), (DEFAULT_SMALL_MODEL.0.to_string(), DEFAULT_SMALL_MODEL.1.to_string()));
        }
    }

    #[test]
    fn prompt_lists_existing_memories_with_ids() {
        let p = build_prompt("I moved to Denver.", &[("fact_a".into(), "User lives in Austin.".into())]);
        assert!(p.contains("- [fact_a] User lives in Austin."));
        assert!(p.contains("I moved to Denver."));
    }

    #[test]
    fn update_supersedes_and_forget_retires() {
        let db = db();
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let old = kernel::persist_facts(&db, "u1", None, &obs, &["User lives in Austin.".into()]).unwrap();
        let other = kernel::persist_facts(&db, "u1", None, &obs, &["User uses Linear.".into()]).unwrap();
        let shown = candidate_facts(&db, "u1", "I moved").unwrap();
        assert_eq!(shown.len(), 2);

        let ops = vec![
            MemoryOp::Update { id: old[0].id.clone(), fact: "User lives in Denver.".into(), memory_type: None },
            MemoryOp::Forget { id: other[0].id.clone() },
            MemoryOp::Add { fact: "What is 2+2?".into(), memory_type: None },
            MemoryOp::Add { fact: "User's API key is sk-123.".into(), memory_type: None },
        ];
        assert_eq!(apply_ops(&db, "u1", None, &obs, &ops, &shown).unwrap(), 3);

        let now: Vec<String> = kernel::list_facts(&db, "u1", None, 50).unwrap().into_iter().map(|f| f.fact).collect();
        assert_eq!(now, vec!["User lives in Denver.".to_string()]);
    }

    #[test]
    fn ids_not_shown_to_the_model_are_untouched() {
        let db = db();
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let kept = kernel::persist_facts(&db, "u1", None, &obs, &["User uses Linear.".into()]).unwrap();
        let ops = vec![MemoryOp::Forget { id: kept[0].id.clone() }];
        assert_eq!(apply_ops(&db, "u1", None, &obs, &ops, &[]).unwrap(), 0);
        assert_eq!(kernel::list_facts(&db, "u1", None, 50).unwrap().len(), 1);
    }

    #[test]
    fn caps_adds_per_turn() {
        let db = db();
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let ops: Vec<MemoryOp> = (0..9).map(|i| MemoryOp::Add { fact: format!("User likes thing number {i}."), memory_type: None }).collect();
        assert_eq!(apply_ops(&db, "u1", None, &obs, &ops, &[]).unwrap(), MAX_ADDS_PER_TURN);
    }
}

/// Live check against a running gizzi-code: `cargo test --lib live_extraction -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn live_extraction() {
        let db = DbHandle::new_memory().unwrap();
        println!("model: {:?}", extraction_model());
        for msg in [
            "What is 17 times 23? Answer in one line.",
            "Reply with just Ready. This is a quick-chat connection check; do not use tools.",
            "I run a small AI consultancy called Allternit and I mostly build in Rust and TypeScript. Can you draft a proposal?",
            "Keep answers short, I hate long preambles.",
            "Actually I switched from Linear to GitHub Projects last month.",
            "My password is hunter2, please log in for me.",
            "Actually I prefer detailed, thorough answers now.",
        ] {
            let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", msg, Some("user")).unwrap();
            extract_and_reconcile(db.clone(), "u1".into(), None, obs, msg.into()).await;
            let facts: Vec<String> = kernel::list_facts(&db, "u1", None, 50).unwrap().into_iter().map(|f| f.fact).collect();
            println!("\n> {msg}\n  memories: {facts:?}");
        }
    }
}
