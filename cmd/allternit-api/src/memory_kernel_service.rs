//! Native Memory Kernel V2 Service
//!
//! Provides additive, lightweight SQLite-backed memory operations:
//! observations, fact extraction, entity tracking, semantic/keyword recall, and turn retention.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::db::DbHandle;
use crate::memory_index::{self, Embedded, Scope, MEMORY_TYPES};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryObservation {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub timestamp: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub fact: String,
    pub confidence: f64,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub source_observation_id: Option<String>,
    /// MEMORY_TYPE (V208): fact / preference / event / procedure / entity /
    /// relationship / task_state. None for facts written before V208.
    #[serde(default)]
    pub memory_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntity {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub entity_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub summary: Option<String>,
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRelationship {
    pub id: String,
    pub user_id: String,
    pub source_entity_id: String,
    pub target_entity_id: String,
    pub relation: String,
    pub confidence: f64,
    pub valid_from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub id: String,
    pub item_type: String, // "fact" | "entity" | "observation"
    pub score: f64,
    pub content: String,
    pub metadata: Value,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordObservationRequest {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetainTurnRequest {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub role: String,
    pub content: String,
    /// The user asked to remember `content` as written (Settings → Memory),
    /// so it is stored as one fact instead of going through extraction.
    #[serde(default)]
    pub explicit: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecallQuery {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub query: String,
    pub limit: Option<usize>,
    /// Retrieve path: let graph expansion return superseded facts too.
    #[serde(default)]
    pub include_history: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum MemoryKernelError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Store or replace the fallback (hash) embedding for a memory target, so the
/// target is searchable by vector at once. The background indexer
/// (`memory_index::spawn_indexer`) re-embeds it with the real local model.
pub fn store_embedding(
    db: &DbHandle,
    user_id: &str,
    target_type: &str,
    target_id: &str,
    text: &str,
) -> Result<String, MemoryKernelError> {
    let emb = memory_index::hash_embed(&[text.to_string()]);
    let conn = db.connect()?;
    memory_index::upsert_embedding(&conn, user_id, target_type, target_id, &emb.model, &emb.vectors[0])?;
    Ok(format!("{}:{}", target_type, target_id))
}

/// Load a single memory target as a RecallResult (current facts only).
fn load_memory_target(
    conn: &rusqlite::Connection,
    target_type: &str,
    target_id: &str,
    agent_id: Option<&str>,
) -> Result<Option<RecallResult>, MemoryKernelError> {
    load_target(conn, target_type, target_id, agent_id, false)
}

/// Load a memory target; `include_history` also returns superseded facts
/// (`valid_until` set, reported in metadata).
pub(crate) fn load_target(
    conn: &rusqlite::Connection,
    target_type: &str,
    target_id: &str,
    agent_id: Option<&str>,
    include_history: bool,
) -> Result<Option<RecallResult>, MemoryKernelError> {
    let agent_ok = |row_agent: &Option<String>| match (row_agent, agent_id) {
        (Some(a), Some(q)) => a == q,
        _ => true,
    };
    let table = match target_type {
        "fact" => "memory_facts",
        "entity" => "memory_entities",
        "observation" => "memory_observations",
        _ => return Ok(None),
    };
    let row_agent: Option<Option<String>> = conn
        .query_row(&format!("SELECT agent_id FROM {table} WHERE id = ?1"), params![target_id], |r| r.get(0))
        .optional()?;
    match row_agent {
        Some(ref a) if agent_ok(a) => {}
        _ => return Ok(None),
    }
    match target_type {
        "fact" => conn
            .query_row(
                "SELECT id, fact, confidence, valid_from, source_observation_id, memory_type, valid_until, decay_score FROM memory_facts
                 WHERE id = ?1 AND (?2 OR valid_until IS NULL)",
                params![target_id, include_history],
                |row| {
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "fact".to_string(),
                        score: 0.0,
                        content: row.get::<_, String>(1)?,
                        metadata: serde_json::json!({
                            "confidence": row.get::<_, f64>(2)?,
                            "source_observation_id": row.get::<_, Option<String>>(4)?,
                            "memory_type": row.get::<_, Option<String>>(5)?,
                            "valid_until": row.get::<_, Option<String>>(6)?,
                            "decay_score": row.get::<_, Option<f64>>(7)?,
                        }),
                        timestamp: row.get::<_, String>(3)?,
                    })
                },
            )
            .optional()
            .map_err(MemoryKernelError::from),
        "entity" => conn
            .query_row(
                "SELECT id, entity_id, name, type, summary, last_updated FROM memory_entities WHERE id = ?1",
                params![target_id],
                |row| {
                    let name: String = row.get(2)?;
                    let etype: String = row.get(3)?;
                    let summary: Option<String> = row.get(4)?;
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "entity".to_string(),
                        score: 0.0,
                        content: format!("[Entity: {} ({})] {}", name, etype, summary.as_deref().unwrap_or("")),
                        metadata: serde_json::json!({
                            "entity_id": row.get::<_, String>(1)?,
                            "name": name,
                            "type": etype,
                            "summary": summary,
                        }),
                        timestamp: row.get::<_, String>(5)?,
                    })
                },
            )
            .optional()
            .map_err(MemoryKernelError::from),
        "observation" => conn
            .query_row(
                "SELECT id, kind, content, timestamp, source FROM memory_observations WHERE id = ?1",
                params![target_id],
                |row| {
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "observation".to_string(),
                        score: 0.0,
                        content: row.get::<_, String>(2)?,
                        metadata: serde_json::json!({
                            "kind": row.get::<_, String>(1)?,
                            "source": row.get::<_, Option<String>>(4)?,
                        }),
                        timestamp: row.get::<_, String>(3)?,
                    })
                },
            )
            .optional()
            .map_err(MemoryKernelError::from),
        _ => Ok(None),
    }
}

/// Memory targets most similar to the query text: hybrid keyword + vector
/// search over the shared index, with the hash query embedding (sync callers;
/// async callers use [`recall_hybrid`] for the real model). Returns
/// (target_type, target_id, fused score), best first.
pub fn recall_semantic(
    db: &DbHandle,
    user_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<(String, String, f64)>, MemoryKernelError> {
    let conn = db.connect()?;
    let q = memory_index::hash_embed(&[query.to_string()]);
    let scope = Scope { scope: user_id, target_types: MEMORY_TYPES, only_ids: None };
    let hits = memory_index::hybrid_search(&conn, &scope, query, Some((&q.model, &q.vectors[0])), limit)?;
    Ok(hits.into_iter().map(|h| (h.target_type, h.target_id, h.score)).collect())
}

/// Record a raw observation (turn, tool execution, file event, decision, or checkpoint).
pub fn record_observation(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    kind: &str,
    content: &str,
    source: Option<&str>,
) -> Result<String, MemoryKernelError> {
    let id = format!("obs_{}", Uuid::new_v4().simple());
    let conn = db.connect()?;

    conn.execute(
        "INSERT INTO memory_observations (id, user_id, agent_id, session_id, kind, content, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, user_id, agent_id, session_id, kind, content, source],
    )?;

    Ok(id)
}

/// Openers of first-person statements that describe the user durably
/// (who they are, what they use or prefer). Lowercased, matched as prefixes.
const SELF_DISCLOSURE_OPENERS: &[&str] = &[
    "i am ", "i'm ", "i work", "i live", "i use ", "i mostly use", "i prefer", "i like ",
    "i love ", "i hate ", "i dislike", "i don't like", "i do not like", "i always",
    "i usually", "i never", "i have ", "i've been", "i was born", "i run ", "i own ",
    "i build", "i manage", "i lead", "i study", "i speak", "i'm based", "my ", "we use ",
    "we are ", "we're ", "our ", "call me ",
];

/// First-person openers that are requests or musings, not facts about the user.
const NON_FACT_OPENERS: &[&str] = &[
    "i'm wondering", "i am wondering", "i'm curious", "i am curious", "i'm trying",
    "i am trying", "i'm looking for", "i am looking for", "i'm asking", "i am asking",
    "i'm not sure", "i am not sure", "i have a question", "i have no idea", "my question",
    "i'm going to", "i am going to", "i'm getting", "i am getting",
];

/// Explicit memory requests; the text after the marker is kept as the fact.
const REMEMBER_MARKERS: &[&str] = &[
    "please remember that ", "please remember ", "remember that ", "remember: ",
    "note that i ", "for future reference, ",
];

/// Split text into sentences on line breaks and terminal punctuation.
fn split_sentences(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let mut current = String::new();
        let chars: Vec<char> = line.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            current.push(*c);
            let at_boundary = matches!(c, '.' | '!' | '?')
                && chars.get(i + 1).map_or(true, |n| n.is_whitespace());
            if at_boundary {
                out.push(std::mem::take(&mut current));
            }
        }
        if !current.trim().is_empty() {
            out.push(current);
        }
    }
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A sentence that looks like markup, code, or a link dump rather than prose.
fn looks_structured(s: &str) -> bool {
    s.contains('|')
        || s.contains("```")
        || s.contains("**")
        || s.contains("http://")
        || s.contains("https://")
        || s.contains('{')
        || s.contains('`')
        || s.starts_with('#')
        || s.starts_with('>')
}

/// Normalise a kept sentence: bullet stripped, capitalised, one full stop.
fn tidy_fact(s: &str) -> String {
    let s = s.trim_start_matches(|c| c == '-' || c == '*' || c == '•').trim();
    let s = s.trim_end_matches(|c| c == '.' || c == '!' || c == ' ');
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Extract durable facts about the user from a message the user wrote.
///
/// Keeps first-person self-descriptions ("I work in Rust", "My team uses
/// Linear") and explicit requests to remember something. Questions,
/// instructions to the assistant, test prompts, and markup are dropped, so a
/// chat turn is never stored verbatim as a memory. Only call this on text the
/// user authored: assistant replies describe the world, not the user.
pub fn extract_facts_heuristic(content: &str) -> Vec<String> {
    let mut facts: Vec<String> = Vec::new();
    for sentence in split_sentences(content) {
        let clean = sentence.trim_start_matches(|c| c == '-' || c == '*' || c == '•').trim();
        if clean.ends_with('?') || looks_structured(clean) {
            continue;
        }
        let lower = clean.to_lowercase().replace('\u{2019}', "'");
        let candidate = if let Some(marker) = REMEMBER_MARKERS.iter().find(|m| lower.starts_with(*m)) {
            // "note that i ..." keeps the "I"; the others drop the marker.
            let keep_from = if *marker == "note that i " { marker.len() - 2 } else { marker.len() };
            clean[keep_from..].to_string()
        } else if SELF_DISCLOSURE_OPENERS.iter().any(|o| lower.starts_with(o))
            && !NON_FACT_OPENERS.iter().any(|o| lower.starts_with(o))
        {
            clean.to_string()
        } else {
            continue;
        };
        let len = candidate.chars().count();
        if !(8..=240).contains(&len) {
            continue;
        }
        let fact = tidy_fact(&candidate);
        if !facts.iter().any(|f| f.eq_ignore_ascii_case(&fact)) {
            facts.push(fact);
        }
    }
    facts
}

/// Save an explicit memory ("remember this" in Settings) as one fact, as
/// written, without the chat-turn filter.
pub fn explicit_fact(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let len = trimmed.chars().count();
    (1..=500).contains(&len).then(|| tidy_fact(trimmed)).filter(|f| !f.is_empty())
}

/// Text that looks like it carries a credential. Such text is never stored
/// as a memory, whichever path produced it.
pub fn mentions_secret(text: &str) -> bool {
    let lower = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "password", "passcode", "passwd", "api key", "api_key", "apikey", "secret key",
        "access token", "private key", "seed phrase", "recovery phrase", "ssn",
        "social security", "card number", "cvv", "pin is", "pin code",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
        || lower.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            ["sk-", "ghp_", "gho_", "xoxb-", "xoxp-", "akia"].iter().any(|p| w.starts_with(p))
                || (w.len() >= 13 && w.len() <= 19 && w.chars().all(|c| c.is_ascii_digit()))
        })
}

/// Persist extracted facts linked to an observation and index embeddings for semantic recall.
pub fn persist_facts(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    observation_id: &str,
    facts: &[String],
) -> Result<Vec<MemoryFact>, MemoryKernelError> {
    let typed: Vec<(String, Option<&str>)> = facts.iter().map(|f| (f.clone(), None)).collect();
    persist_facts_typed(db, user_id, agent_id, observation_id, &typed, &[])
}

/// Persist facts (with optional memory types) and retire `retire` fact ids in
/// one step. With a Memory Drive this is ONE git commit, after which the rows
/// are rebuilt from the drive; without one it is the legacy row writer.
pub fn persist_facts_typed(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    observation_id: &str,
    facts: &[(String, Option<&str>)],
    retire: &[String],
) -> Result<Vec<MemoryFact>, MemoryKernelError> {
    let adds: Vec<crate::memory_drive_writer::NewFact> = facts
        .iter()
        .map(|(text, t)| crate::memory_drive_writer::NewFact {
            text: text.clone(),
            memory_type: t.map(str::to_string),
            agent: agent_id.map(str::to_string),
            observation: (!observation_id.is_empty()).then(|| observation_id.to_string()),
            ..Default::default()
        })
        .collect();
    match crate::memory_drive_writer::commit_facts(db, user_id, &adds, retire, "Remember from conversation") {
        Ok(Some(out)) => {
            let conn = db.connect()?;
            for legacy in &out.legacy_retire {
                crate::memory_relations::supersede_fact(&conn, user_id, legacy)?;
            }
            let mut persisted = Vec::new();
            for ((text, t), id) in facts.iter().zip(out.fact_ids) {
                if let Some(id) = id {
                    persisted.push(MemoryFact {
                        id,
                        user_id: user_id.to_string(),
                        agent_id: agent_id.map(|s| s.to_string()),
                        fact: text.clone(),
                        confidence: 0.85,
                        valid_from: chrono::Utc::now().to_rfc3339(),
                        valid_until: None,
                        source_observation_id: Some(observation_id.to_string()),
                        memory_type: t.map(str::to_string),
                    });
                }
            }
            return Ok(persisted);
        }
        Ok(None) => {}
        Err(e) => return Err(MemoryKernelError::Internal(e.to_string())),
    }
    let conn = db.connect()?;
    for id in retire {
        crate::memory_relations::supersede_fact(&conn, user_id, id)?;
    }
    let mut persisted = Vec::new();

    for (fact, _) in facts {
        let fact = fact.as_str();
        if mentions_secret(fact) {
            continue;
        }
        // The same fact restated in a later turn is not a new memory.
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_facts
             WHERE user_id = ?1 AND lower(fact) = lower(?2) AND valid_until IS NULL)",
            params![user_id, fact],
            |row| row.get(0),
        )?;
        if exists {
            continue;
        }
        let fact_id = format!("fact_{}", Uuid::new_v4().simple());
        conn.execute(
            "INSERT INTO memory_facts (id, user_id, agent_id, fact, confidence, source_observation_id)
             VALUES (?1, ?2, ?3, ?4, 0.85, ?5)",
            params![fact_id, user_id, agent_id, fact, observation_id],
        )?;

        // Best-effort embedding index; failures should not block fact persistence.
        let _ = store_embedding(db, user_id, "fact", &fact_id, fact);

        persisted.push(MemoryFact {
            id: fact_id,
            user_id: user_id.to_string(),
            agent_id: agent_id.map(|s| s.to_string()),
            fact: fact.to_string(),
            confidence: 0.85,
            valid_from: chrono::Utc::now().to_rfc3339(),
            valid_until: None,
            source_observation_id: Some(observation_id.to_string()),
            memory_type: None,
        });
    }

    Ok(persisted)
}

/// Retain an agent/user turn: logs an observation and extracts facts about
/// the user from what the user wrote. Assistant and tool turns are kept as
/// observations only. Returns the observation id and how many facts were saved.
pub fn retain_turn(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    role: &str,
    content: &str,
    explicit: bool,
) -> Result<(String, usize), MemoryKernelError> {
    let kind = if explicit { "explicit_memory".to_string() } else { format!("turn_{}", role) };
    let obs_id = record_observation(db, user_id, agent_id, session_id, &kind, content, Some(role))?;

    let facts = if explicit {
        explicit_fact(content).into_iter().collect()
    } else if role == "user" {
        extract_facts_heuristic(content)
    } else {
        Vec::new()
    };
    let saved = if facts.is_empty() {
        0
    } else {
        persist_facts(db, user_id, agent_id, &obs_id, &facts).map(|p| p.len()).unwrap_or(0)
    };

    Ok((obs_id, saved))
}

/// Remove facts the old extractor made from raw chat turns: anything taken
/// from an assistant turn or a session dream, and user-turn facts that the
/// current extractor would not produce. Explicit memories are left alone.
/// Runs once, at the first startup after upgrade.
pub fn prune_turn_derived_facts(db: &DbHandle) -> Result<usize, MemoryKernelError> {
    let conn = db.connect()?;
    // One-time: later facts from user turns may be model-written, which the
    // rule-based check below would wrongly reject.
    let done: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memory_maintenance WHERE key = 'prune_turn_facts_v1')",
        [],
        |row| row.get(0),
    )?;
    if done {
        return Ok(0);
    }
    let mut stmt = conn.prepare(
        "SELECT f.id, f.fact, o.kind FROM memory_facts f
         JOIN memory_observations o ON o.id = f.source_observation_id
         WHERE o.kind IN ('turn_user', 'turn_assistant', 'turn_tool', 'turn_system', 'dream_extraction')
           AND NOT EXISTS(SELECT 1 FROM memory_drive_entries e WHERE e.fact_id = f.id)",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut removed = 0;
    for (id, fact, kind) in rows {
        let keep = kind == "turn_user" && extract_facts_heuristic(&fact).len() == 1
            && extract_facts_heuristic(&fact)[0].eq_ignore_ascii_case(&tidy_fact(&fact));
        if keep {
            continue;
        }
        conn.execute(
            "DELETE FROM memory_embeddings WHERE target_type = 'fact' AND target_id = ?1",
            params![id],
        )?;
        removed += conn.execute("DELETE FROM memory_facts WHERE id = ?1", params![id])?;
    }
    conn.execute("INSERT INTO memory_maintenance (key) VALUES ('prune_turn_facts_v1')", [])?;
    Ok(removed)
}

/// Recall memories matching a query across facts, entities, and observations
/// (sync; hash query embedding). Prefer [`recall_hybrid`] from async code.
pub fn recall(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<RecallResult>, MemoryKernelError> {
    let q = memory_index::hash_embed(&[query.to_string()]);
    recall_with_embedding(db, user_id, agent_id, session_id, query, Some(&q), limit)
}

/// Recall with the query embedded by the local embedding endpoint (hash
/// fallback when it is down).
pub async fn recall_hybrid(
    db: &DbHandle,
    client: &memory_index::EmbedClient,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<RecallResult>, MemoryKernelError> {
    let q = client.embed_or_hash(&[query.to_string()], memory_index::InputType::Query).await;
    let (db, user_id, agent_id, session_id, query) = (
        db.clone(),
        user_id.to_string(),
        agent_id.map(str::to_string),
        session_id.map(str::to_string),
        query.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        recall_with_embedding(&db, &user_id, agent_id.as_deref(), session_id.as_deref(), &query, Some(&q), limit)
    })
    .await
    .map_err(|e| MemoryKernelError::Internal(e.to_string()))?
}

/// [`recall_hybrid`] that also returns the recall-log id (the join key of
/// the retrieve-path S1 shadow) and the query embedding it used.
pub async fn recall_hybrid_logged(
    db: &DbHandle,
    client: &memory_index::EmbedClient,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<(Vec<RecallResult>, String, Embedded), MemoryKernelError> {
    let q = client.embed_or_hash(&[query.to_string()], memory_index::InputType::Query).await;
    let (db, user_id, agent_id, session_id, query, qc) = (
        db.clone(),
        user_id.to_string(),
        agent_id.map(str::to_string),
        session_id.map(str::to_string),
        query.to_string(),
        q.clone(),
    );
    let (results, log_id) = tokio::task::spawn_blocking(move || {
        recall_logged(&db, &user_id, agent_id.as_deref(), session_id.as_deref(), &query, Some(&qc), limit)
    })
    .await
    .map_err(|e| MemoryKernelError::Internal(e.to_string()))??;
    Ok((results, log_id, q))
}

/// Hybrid recall: FTS5 keyword + vector candidates from the shared memory
/// index, fused with reciprocal rank fusion, weighted by item type and
/// (lightly) recency. An empty query returns the most recent facts/entities.
pub fn recall_with_embedding(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    query_embedding: Option<&Embedded>,
    limit: usize,
) -> Result<Vec<RecallResult>, MemoryKernelError> {
    recall_logged(db, user_id, agent_id, session_id, query, query_embedding, limit).map(|(r, _)| r)
}

/// [`recall_with_embedding`] returning the `memory_recall_logs` id too.
pub fn recall_logged(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    query_embedding: Option<&Embedded>,
    limit: usize,
) -> Result<(Vec<RecallResult>, String), MemoryKernelError> {
    let conn = db.connect()?;
    let mut results: Vec<RecallResult> = Vec::new();

    if memory_index::fts_query(query).is_none() {
        // Nothing to search for: most recent facts, then entities.
        let mut stmt = conn.prepare(
            "SELECT 'fact', id FROM memory_facts
             WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL) AND valid_until IS NULL
             UNION ALL
             SELECT 'entity', id FROM memory_entities
             WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
             LIMIT ?3",
        )?;
        let keys: Vec<(String, String)> = stmt
            .query_map(params![user_id, agent_id, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (t, id) in keys {
            if let Some(mut hit) = load_memory_target(&conn, &t, &id, agent_id)? {
                hit.score = hit.metadata["confidence"].as_f64().unwrap_or(0.5);
                results.push(hit);
            }
        }
    } else {
        let scope = Scope { scope: user_id, target_types: MEMORY_TYPES, only_ids: None };
        let qv = query_embedding.and_then(|e| e.vectors.first().map(|v| (e.model.as_str(), v.as_slice())));
        let hits = memory_index::hybrid_search(&conn, &scope, query, qv, limit.max(10) * 3)?;
        let now = chrono::Utc::now();
        for h in hits {
            let Some(mut item) = load_memory_target(&conn, &h.target_type, &h.target_id, agent_id)? else {
                continue;
            };
            let type_weight = match item.item_type.as_str() {
                "fact" => 1.2,
                "entity" => 1.0,
                _ => 0.8,
            };
            // Recency only breaks near-ties: 72h half-life, at most 15%.
            let recency = parse_ts(&item.timestamp)
                .map(|t| (-((now - t).num_hours().max(0) as f64) / 72.0).exp())
                .unwrap_or(0.5);
            // Decayed facts (WP-M1d) rank at half weight until retrieved again.
            let decay = if item.metadata["decay_score"].is_null() { 1.0 } else { 0.5 };
            item.score = h.score * type_weight * (0.85 + 0.15 * recency) * decay;
            item.metadata["keyword_rank"] = serde_json::json!(h.keyword_rank);
            item.metadata["vector_rank"] = serde_json::json!(h.vector_rank);
            if let Some(sim) = h.similarity {
                item.metadata["semantic_similarity"] = serde_json::json!(sim);
            }
            if let Some(e) = query_embedding {
                item.metadata["embedding_model"] = serde_json::json!(e.model);
            }
            results.push(item);
        }
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }
    results.truncate(limit);

    // Retrieval stats feed consolidation's decay (WP-M1d).
    let fact_ids: Vec<&str> = results.iter().filter(|r| r.item_type == "fact").map(|r| r.id.as_str()).collect();
    if let Err(e) = crate::memory_consolidation::note_retrieved(&conn, user_id, &fact_ids) {
        tracing::debug!(error = %e, "memory retrieval stats not recorded");
    }

    // Record recall log
    let log_id = format!("rec_{}", Uuid::new_v4().simple());
    let results_json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
    let _ = conn.execute(
        "INSERT INTO memory_recall_logs (id, user_id, agent_id, session_id, query, results)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![log_id, user_id, agent_id, session_id, query, results_json],
    );

    Ok((results, log_id))
}

/// Timestamps come back as RFC 3339 or SQLite's `YYYY-MM-DD HH:MM:SS`.
fn parse_ts(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|n| n.and_utc())
        })
}

/// Cosine similarity helper between two float vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// List recent observations.
pub fn list_observations(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryObservation>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, agent_id, session_id, kind, content, timestamp, source
         FROM memory_observations
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY timestamp DESC
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![user_id, agent_id, limit as i64], |row| {
        Ok(MemoryObservation {
            id: row.get(0)?,
            user_id: row.get(1)?,
            agent_id: row.get(2)?,
            session_id: row.get(3)?,
            kind: row.get(4)?,
            content: row.get(5)?,
            timestamp: row.get(6)?,
            source: row.get(7)?,
        })
    })?;

    let mut observations = Vec::new();
    for obs in rows.flatten() {
        observations.push(obs);
    }
    Ok(observations)
}

/// List recent facts.
/// Delete one of the user's facts and its embedding. Returns whether a row was removed.
pub fn delete_fact(db: &DbHandle, user_id: &str, fact_id: &str) -> Result<bool, MemoryKernelError> {
    let conn = db.connect()?;
    // Drive-backed: remove the entry from the drive first (one commit), then
    // drop the index row. Git history keeps the old line, as with any edit.
    let owned_drive_fact: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memory_drive_entries e JOIN memory_drives d ON d.id=e.drive_id
          WHERE e.fact_id=?1 AND d.user_id=?2 AND d.kind='personal')",
        params![fact_id, user_id],
        |r| r.get(0),
    )?;
    if owned_drive_fact {
        crate::memory_drive_writer::commit_facts(db, user_id, &[], &[fact_id.to_string()], "Forget memory")
            .map_err(|e| MemoryKernelError::Internal(e.to_string()))?;
    }
    let removed = conn.execute(
        "DELETE FROM memory_facts WHERE id = ?1 AND user_id = ?2",
        params![fact_id, user_id],
    )?;
    if removed > 0 {
        conn.execute(
            "DELETE FROM memory_embeddings WHERE user_id = ?1 AND target_type = 'fact' AND target_id = ?2",
            params![user_id, fact_id],
        )?;
    }
    Ok(removed > 0)
}

pub fn list_facts(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryFact>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, agent_id, fact, confidence, valid_from, valid_until, source_observation_id, memory_type
         FROM memory_facts
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
           AND valid_until IS NULL
         ORDER BY valid_from DESC
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![user_id, agent_id, limit as i64], |row| {
        Ok(MemoryFact {
            id: row.get(0)?,
            user_id: row.get(1)?,
            agent_id: row.get(2)?,
            fact: row.get(3)?,
            confidence: row.get(4)?,
            valid_from: row.get(5)?,
            valid_until: row.get(6)?,
            source_observation_id: row.get(7)?,
            memory_type: row.get(8)?,
        })
    })?;

    let mut facts = Vec::new();
    for f in rows.flatten() {
        facts.push(f);
    }
    Ok(facts)
}

/// List entities.
pub fn list_entities(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryEntity>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, agent_id, entity_id, name, type, summary, last_updated
         FROM memory_entities
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY last_updated DESC
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![user_id, agent_id, limit as i64], |row| {
        Ok(MemoryEntity {
            id: row.get(0)?,
            user_id: row.get(1)?,
            agent_id: row.get(2)?,
            entity_id: row.get(3)?,
            name: row.get(4)?,
            entity_type: row.get(5)?,
            summary: row.get(6)?,
            last_updated: row.get(7)?,
        })
    })?;

    let mut entities = Vec::new();
    for e in rows.flatten() {
        entities.push(e);
    }
    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v1 = vec![1.0, 2.0, 3.0];
        let v2 = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v1, &v2);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let v1 = vec![1.0, 0.0];
        let v2 = vec![0.0, 1.0];
        let sim = cosine_similarity(&v1, &v2);
        assert!((sim - 0.0).abs() < 1e-5);
    }

    #[test]
    fn extracts_first_person_facts() {
        let facts = extract_facts_heuristic(
            "I'm a product designer in Austin. My team uses Linear for tracking. Can you help me plan a sprint?",
        );
        assert_eq!(facts, vec!["I'm a product designer in Austin.", "My team uses Linear for tracking."]);
    }

    #[test]
    fn keeps_explicit_remember_requests() {
        let facts = extract_facts_heuristic("Please remember that invoices go out on the 1st.");
        assert_eq!(facts, vec!["Invoices go out on the 1st."]);
    }

    #[test]
    fn drops_questions_instructions_and_replies() {
        // Real turns the old extractor stored as "facts".
        for turn in [
            "Reply with just Ready. This is a quick-chat connection check; do not use tools.",
            "What is 17 times 23? Answer in one line.",
            "Bitcoin is currently trading at roughly **$84,000 USD** (around $83,900).",
            "| **Ease of use** | Steepest learning curve; console and IAM are dense |",
            "I'm wondering which cloud is cheapest.",
            "Goal: Build a high-performance bot.",
        ] {
            assert!(extract_facts_heuristic(turn).is_empty(), "stored: {turn}");
        }
    }

    #[test]
    fn secrets_are_never_stored() {
        let db = test_db();
        for text in ["My password is hunter2.", "My key is sk-abc123def", "My card is 4111111111111111"] {
            let (_, n) = retain_turn(&db, "u1", None, None, "user", text, false).unwrap();
            assert_eq!(n, 0, "stored: {text}");
            let (_, n) = retain_turn(&db, "u1", None, None, "user", text, true).unwrap();
            assert_eq!(n, 0, "stored explicit: {text}");
        }
        assert!(!mentions_secret("I work in Rust and TypeScript."));
    }

    #[test]
    fn explicit_fact_is_kept_as_written() {
        assert_eq!(explicit_fact("  prefers dark mode ").as_deref(), Some("Prefers dark mode."));
        assert_eq!(explicit_fact("   "), None);
    }

    fn test_db() -> DbHandle {
        DbHandle::new_memory().expect("memory db")
    }

    #[test]
    fn assistant_turns_yield_no_facts_and_repeats_are_deduped() {
        let db = test_db();
        let (_, n) = retain_turn(&db, "u1", None, None, "assistant", "My name is Claude and I am helpful.", false).unwrap();
        assert_eq!(n, 0);
        let (_, n) = retain_turn(&db, "u1", None, None, "user", "I work in Rust.", false).unwrap();
        assert_eq!(n, 1);
        let (_, n) = retain_turn(&db, "u1", None, None, "user", "i work in rust", false).unwrap();
        assert_eq!(n, 0);
        let (_, n) = retain_turn(&db, "u1", None, None, "user", "Prefers dark mode", true).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn prune_removes_legacy_turn_facts_only() {
        let db = test_db();
        let obs_user = record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let obs_asst = record_observation(&db, "u1", None, None, "turn_assistant", "x", Some("assistant")).unwrap();
        let obs_explicit = record_observation(&db, "u1", None, None, "explicit_memory", "x", Some("user")).unwrap();
        persist_facts(&db, "u1", None, &obs_user, &["What is 17 times 23? Answer in one line.".into()]).unwrap();
        persist_facts(&db, "u1", None, &obs_user, &["I work in Rust.".into()]).unwrap();
        persist_facts(&db, "u1", None, &obs_asst, &["Bitcoin is trading at $84,000.".into()]).unwrap();
        persist_facts(&db, "u1", None, &obs_explicit, &["Prefers dark mode.".into()]).unwrap();

        assert_eq!(prune_turn_derived_facts(&db).unwrap(), 2);
        let left: Vec<String> = list_facts(&db, "u1", None, 50).unwrap().into_iter().map(|f| f.fact).collect();
        assert_eq!(left.len(), 2);
        assert!(left.contains(&"I work in Rust.".to_string()));
        assert!(left.contains(&"Prefers dark mode.".to_string()));
        assert_eq!(prune_turn_derived_facts(&db).unwrap(), 0);
    }

    #[test]
    fn embedding_byte_roundtrip() {
        let vec = vec![1.0f32, -2.5, 3.75, 0.0];
        let bytes = memory_index::f32_to_bytes(&vec);
        let restored = memory_index::bytes_to_f32(&bytes);
        assert_eq!(vec, restored);
    }

    #[test]
    fn local_embedding_has_configured_dimensions() {
        let emb = memory_index::hash_embed(&["hello world".to_string()]);
        assert_eq!(emb.vectors[0].len(), memory_index::HASH_DIM);
        assert_eq!(emb.dim, memory_index::HASH_DIM);
    }

    #[test]
    fn migration_stack_adds_memory_entities_summary() {
        // Boots the full embedded migration chain on a scratch DB and asserts
        // the V183 repair landed: memory_entities.summary must be selectable.
        let db = DbHandle::new_memory().expect("migration stack should boot");
        let conn = db.connect().expect("connect");
        let mut stmt = conn
            .prepare("SELECT summary FROM memory_entities LIMIT 1")
            .expect("memory_entities.summary must exist after migrations");
        let _ = stmt.query([]).expect("summary select should run");
    }
}
