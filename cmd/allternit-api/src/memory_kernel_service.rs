//! Native Memory Kernel V2 Service
//!
//! Provides additive, lightweight SQLite-backed memory operations:
//! observations, fact extraction, entity tracking, semantic/keyword recall, and turn retention.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::db::DbHandle;
use crate::llm_gateway::embeddings::generate_local_embedding;

const EMBEDDING_DIM: usize = 384;

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

fn f32_vec_to_bytes(vec: &[f32]) -> Vec<u8> {
    vec.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(chunk);
            f32::from_le_bytes(arr)
        })
        .collect()
}

/// Store or replace a local embedding for a memory target.
pub fn store_embedding(
    db: &DbHandle,
    user_id: &str,
    target_type: &str,
    target_id: &str,
    text: &str,
) -> Result<String, MemoryKernelError> {
    let id = format!("emb_{}", Uuid::new_v4().simple());
    let embedding = generate_local_embedding(text, EMBEDDING_DIM);
    let bytes = f32_vec_to_bytes(&embedding);
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO memory_embeddings (id, user_id, target_type, target_id, embedding, model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(user_id, target_type, target_id)
         DO UPDATE SET embedding = excluded.embedding, model = excluded.model, created_at = CURRENT_TIMESTAMP",
        params![id, user_id, target_type, target_id, bytes, "local-hash-384"],
    )?;
    Ok(id)
}

/// Load a single memory target as a RecallResult.
fn load_memory_target(
    conn: &rusqlite::Connection,
    target_type: &str,
    target_id: &str,
) -> Result<Option<RecallResult>, MemoryKernelError> {
    match target_type {
        "fact" => conn
            .query_row(
                "SELECT id, fact, confidence, valid_from, source_observation_id FROM memory_facts WHERE id = ?1 AND valid_until IS NULL",
                params![target_id],
                |row| {
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "fact".to_string(),
                        score: 0.0,
                        content: row.get::<_, String>(1)?,
                        metadata: serde_json::json!({
                            "confidence": row.get::<_, f64>(2)?,
                            "source_observation_id": row.get::<_, Option<String>>(3)?,
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

/// Find memory targets whose embeddings are most similar to the query text.
pub fn recall_semantic(
    db: &DbHandle,
    user_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<(String, String, f64)>, MemoryKernelError> {
    let conn = db.connect()?;
    let query_vec = generate_local_embedding(query, EMBEDDING_DIM);
    let mut stmt = conn.prepare(
        "SELECT target_type, target_id, embedding FROM memory_embeddings WHERE user_id = ?1",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;

    let mut scored: Vec<(String, String, f64)> = Vec::new();
    for row in rows.flatten() {
        let (target_type, target_id, bytes) = row;
        let candidate = bytes_to_f32_vec(&bytes);
        if candidate.len() == query_vec.len() {
            let sim = cosine_similarity(&query_vec, &candidate) as f64;
            if sim > 0.0 {
                scored.push((target_type, target_id, sim));
            }
        }
    }
    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
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
    let conn = db.connect()?;
    let mut persisted = Vec::new();

    for fact in facts {
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
         WHERE o.kind IN ('turn_user', 'turn_assistant', 'turn_tool', 'turn_system', 'dream_extraction')",
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

/// Recall memories matching a query across facts, entities, and recent observations.
pub fn recall(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<RecallResult>, MemoryKernelError> {
    let conn = db.connect()?;
    let words: Vec<String> = query
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .map(|w| format!("%{}%", w.to_lowercase()))
        .collect();

    let mut results: Vec<RecallResult> = Vec::new();

    // 1. Search facts
    let mut fact_stmt = conn.prepare(
        "SELECT id, fact, confidence, valid_from, source_observation_id
         FROM memory_facts
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
           AND valid_until IS NULL
         ORDER BY valid_from DESC
         LIMIT 50",
    )?;

    let fact_rows = fact_stmt.query_map(params![user_id, agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    for row in fact_rows.flatten() {
        let (id, fact_text, confidence, valid_from, src_obs) = row;
        let lower = fact_text.to_lowercase();
        let match_count = words.iter().filter(|&w| lower.contains(&w[1..w.len() - 1])).count();
        let score = if words.is_empty() {
            confidence
        } else {
            (match_count as f64 / words.len().max(1) as f64) * confidence + 0.1
        };

        if score > 0.1 || words.is_empty() {
            results.push(RecallResult {
                id,
                item_type: "fact".to_string(),
                score,
                content: fact_text,
                metadata: serde_json::json!({
                    "confidence": confidence,
                    "source_observation_id": src_obs,
                }),
                timestamp: valid_from,
            });
        }
    }

    // 2. Search entities
    let mut entity_stmt = conn.prepare(
        "SELECT id, entity_id, name, type, summary, last_updated
         FROM memory_entities
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY last_updated DESC
         LIMIT 30",
    )?;

    let entity_rows = entity_stmt.query_map(params![user_id, agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    for row in entity_rows.flatten() {
        let (id, entity_id, name, etype, summary, updated) = row;
        let search_text = format!("{} {} {}", name, etype, summary.as_deref().unwrap_or("")).to_lowercase();
        let match_count = words.iter().filter(|&w| search_text.contains(&w[1..w.len() - 1])).count();
        let score = if words.is_empty() {
            0.5
        } else {
            (match_count as f64 / words.len().max(1) as f64) * 0.9
        };

        if score > 0.1 || words.is_empty() {
            results.push(RecallResult {
                id,
                item_type: "entity".to_string(),
                score,
                content: format!("[Entity: {} ({})] {}", name, etype, summary.as_deref().unwrap_or("")),
                metadata: serde_json::json!({
                    "entity_id": entity_id,
                    "name": name,
                    "type": etype,
                }),
                timestamp: updated,
            });
        }
    }

    // 3. Search observations (fallback/recent context)
    let mut obs_stmt = conn.prepare(
        "SELECT id, kind, content, timestamp, source
         FROM memory_observations
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY timestamp DESC
         LIMIT 20",
    )?;

    let obs_rows = obs_stmt.query_map(params![user_id, agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    for row in obs_rows.flatten() {
        let (id, kind, content, ts, src) = row;
        let lower = content.to_lowercase();
        let match_count = words.iter().filter(|&w| lower.contains(&w[1..w.len() - 1])).count();
        let score = if words.is_empty() {
            0.3
        } else {
            (match_count as f64 / words.len().max(1) as f64) * 0.7
        };

        if score > 0.15 {
            results.push(RecallResult {
                id,
                item_type: "observation".to_string(),
                score,
                content,
                metadata: serde_json::json!({
                    "kind": kind,
                    "source": src,
                }),
                timestamp: ts,
            });
        }
    }

    // 4. Augment with semantic/embedding recall when embeddings exist.
    let semantic_hits = recall_semantic(db, user_id, query, limit.max(20))?;
    if !semantic_hits.is_empty() {
        for (target_type, target_id, sim) in semantic_hits {
            let key = format!("{}:{}", target_type, target_id);
            if let Some(pos) = results.iter().position(|r| format!("{}:{}", r.item_type, r.id) == key) {
                // Boost existing keyword result with semantic signal.
                results[pos].score += sim * 0.35;
                results[pos].metadata["semantic_similarity"] = serde_json::json!(sim);
            } else if let Some(mut hit) = load_memory_target(&conn, &target_type, &target_id)? {
                hit.score = sim * 0.35;
                hit.metadata["semantic_similarity"] = serde_json::json!(sim);
                results.push(hit);
            }
        }
    }

    // 5-Way Reciprocal Rank Fusion (RRF):
    // Blend Lexical (0.25), Confidence/Semantic (0.35), Graph Entity (0.20), and Recency (0.20)
    let k = 60.0;
    let now = chrono::Utc::now();
    for (rank, item) in results.iter_mut().enumerate() {
        let rank_score = 1.0 / (k + (rank as f64) + 1.0);
        let time_score = if let Ok(parsed_ts) = chrono::DateTime::parse_from_rfc3339(&item.timestamp) {
            let age_hours = (now - parsed_ts.with_timezone(&chrono::Utc)).num_hours().max(0) as f64;
            // Half-life decay over 72 hours
            (-age_hours / 72.0).exp()
        } else {
            0.5
        };

        let type_weight = match item.item_type.as_str() {
            "fact" => 1.2,
            "entity" => 1.0,
            _ => 0.8,
        };

        // Reciprocal Rank Fusion formula
        item.score = (item.score * 0.35 + rank_score * 0.25 + time_score * 0.20) * type_weight;
    }

    // Sort by final fused RRF score descending, then by timestamp
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);

    // Record recall log
    let log_id = format!("rec_{}", Uuid::new_v4().simple());
    let results_json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
    let _ = conn.execute(
        "INSERT INTO memory_recall_logs (id, user_id, agent_id, session_id, query, results)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![log_id, user_id, agent_id, session_id, query, results_json],
    );

    Ok(results)
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
        "SELECT id, user_id, agent_id, fact, confidence, valid_from, valid_until, source_observation_id
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
        let bytes = f32_vec_to_bytes(&vec);
        let restored = bytes_to_f32_vec(&bytes);
        assert_eq!(vec, restored);
    }

    #[test]
    fn local_embedding_has_configured_dimensions() {
        let emb = generate_local_embedding("hello world", EMBEDDING_DIM);
        assert_eq!(emb.len(), EMBEDDING_DIM);
    }
}
