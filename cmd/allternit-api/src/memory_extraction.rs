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

use rusqlite::params;
use serde::Deserialize;
use std::sync::OnceLock;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::db::DbHandle;
use crate::memory_kernel_service::{self as kernel, MemoryKernelError};

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
    Add { fact: String },
    Update { id: String, fact: String },
    Forget { id: String },
}

/// Extraction runs through the Claude CLI provider in gizzi-code.
const DEFAULT_MEMORY_MODEL: (&str, &str) = ("claude-cli", "claude-sonnet-5");

/// The model used for extraction: `ALLTERNIT_MEMORY_MODEL` ("provider/model")
/// when set, otherwise Claude CLI.
pub fn extraction_model() -> (String, String) {
    std::env::var("ALLTERNIT_MEMORY_MODEL")
        .ok()
        .and_then(|raw| raw.trim().split_once('/').map(|(p, m)| (p.to_string(), m.to_string())))
        .unwrap_or_else(|| (DEFAULT_MEMORY_MODEL.0.to_string(), DEFAULT_MEMORY_MODEL.1.to_string()))
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

Current memories:
{memories}

Latest user message:
"""
{message}
"""

Return JSON only, in this shape:
{{"operations": [
  {{"op": "add", "fact": "..."}},
  {{"op": "update", "id": "<id of the memory it replaces>", "fact": "..."}},
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
    // Unknown ops are dropped instead of failing the whole reply.
    let value: serde_json::Value = serde_json::from_str(slice).ok()?;
    let ops = value.get("operations")?.as_array()?;
    Some(
        ops.iter()
            .filter_map(|op| serde_json::from_value::<MemoryOp>(op.clone()).ok())
            .collect(),
    )
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

/// End a fact's validity (update or forget). Only facts shown to the model
/// may be touched, so a hallucinated id cannot erase an unrelated memory.
fn invalidate(
    db: &DbHandle,
    user_id: &str,
    id: &str,
    shown: &[(String, String)],
) -> Result<bool, MemoryKernelError> {
    if !shown.iter().any(|(sid, _)| sid == id) {
        return Ok(false);
    }
    let conn = db.connect()?;
    let changed = conn.execute(
        "UPDATE memory_facts SET valid_until = CURRENT_TIMESTAMP
         WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
        params![id, user_id],
    )?;
    if changed > 0 {
        conn.execute(
            "DELETE FROM memory_embeddings WHERE user_id = ?1 AND target_type = 'fact' AND target_id = ?2",
            params![user_id, id],
        )?;
    }
    Ok(changed > 0)
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
    let mut changed = 0;
    let mut adds = Vec::new();
    for op in ops {
        match op {
            MemoryOp::Add { fact } => {
                if let Some(fact) = clean_fact(fact) {
                    if adds.len() < MAX_ADDS_PER_TURN {
                        adds.push(fact);
                    }
                }
            }
            MemoryOp::Update { id, fact } => {
                if let Some(fact) = clean_fact(fact) {
                    if invalidate(db, user_id, id, shown)? {
                        changed += 1;
                        adds.push(fact);
                    }
                }
            }
            MemoryOp::Forget { id } => {
                if invalidate(db, user_id, id, shown)? {
                    changed += 1;
                }
            }
        }
    }
    if !adds.is_empty() {
        changed += kernel::persist_facts(db, user_id, agent_id, observation_id, &adds)?.len();
    }
    Ok(changed)
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
        crate::gizzi_completion::complete_ephemeral(&build_prompt(&message, &shown), Some(SYSTEM_PROMPT), Some(&model)),
    )
    .await;

    let result = tokio::task::spawn_blocking(move || {
        let agent = agent_id.as_deref();
        match reply.as_deref().and_then(parse_ops) {
            Some(ops) => apply_ops(&db, &user_id, agent, &observation_id, &ops, &shown),
            None => {
                let facts = kernel::extract_facts_heuristic(&message);
                if facts.is_empty() {
                    Ok(0)
                } else {
                    kernel::persist_facts(&db, &user_id, agent, &observation_id, &facts).map(|p| p.len())
                }
            }
        }
    })
    .await;

    match result {
        Ok(Ok(n)) => debug!(changed = n, "memory extraction applied"),
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
                MemoryOp::Add { fact: "User works in Rust.".into() },
                MemoryOp::Forget { id: "fact_1".into() },
            ]
        );
        assert_eq!(parse_ops("{\"operations\": []}").unwrap(), vec![]);
        assert!(parse_ops("no json here").is_none());
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
            MemoryOp::Update { id: old[0].id.clone(), fact: "User lives in Denver.".into() },
            MemoryOp::Forget { id: other[0].id.clone() },
            MemoryOp::Add { fact: "What is 2+2?".into() },
            MemoryOp::Add { fact: "User's API key is sk-123.".into() },
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
        let ops: Vec<MemoryOp> = (0..9).map(|i| MemoryOp::Add { fact: format!("User likes thing number {i}.") }).collect();
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
