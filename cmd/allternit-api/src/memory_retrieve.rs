//! Memory retrieve path, decision M1 (WP-M1c): S1 ROUTE → S0 anchors →
//! S1 IS_RELEVANT evidence loop → S0 graph expansion → S1 RANK → evidence
//! with provenance. One S2 synthesis happens in the caller; nothing here
//! generates text.
//!
//! - [`retrieve`] runs the pipeline and returns [`Evidence`] items, each with
//!   the step that selected it (`anchor:<space>` / `expansion:<relation>`)
//!   and the S1 decision ids behind it.
//! - [`shadow_recall`] runs it in SHADOW next to the incumbent (the hybrid
//!   recall in `memory_kernel_service::recall_logged`): the caller always gets
//!   the incumbent's set; the pipeline's would-be set is written to
//!   `memory_s1_decisions` (keyed by the recall-log id) and logged.
//! - [`label_from_answer`] turns the assistant's reply into outcome labels:
//!   evidence the incumbent showed the model is `used` when the reply
//!   restates it (lexical overlap, weak label, source `memory.answer_used`),
//!   else `unused`; items only the pipeline found were never shown, so they
//!   get `not_shown` and no S1 outcome.
//!
//! Spaces: facts / entities / observations / documents (index chunks of
//! memory documents) / procedures (`procedural_memory`) / notes
//! (`memory_notes`). Procedures and notes are not in the shared index yet, so
//! their anchors are an S0 token-overlap scan (WP-M1d moves them into it).
//!
//! When S1 is down every S1 step falls back to S0 order (route = the
//! incumbent's spaces, every anchor kept, rank = anchor order), so the eval
//! harness also measures the S0-only pipeline.
//!
//! Ledger rows (`memory_s1_decisions`, no migration; `observation_id` = recall
//! log id, `candidate_fact_id` = the memory judged):
//!
//! | bank                 | s1_answer                        | incumbent_label            |
//! |----------------------|----------------------------------|----------------------------|
//! | `memory.route`       | the space(s) chosen              | `facts,entities,observations` |
//! | `memory.is_relevant` | `relevant` / `not_relevant`      | `returned` / `not_returned` |
//! | `memory.rank`        | `high` / `medium` / `low`        | `returned` / `not_returned` |
//! | `memory.retrieve`    | the selecting step (would-be set)| `returned` / `not_returned` |
//!
//! `user_label` is filled by [`label_from_answer`]: `used` / `unused` /
//! `not_shown` (and the used space on the route row).
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Instant;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tracing::{debug, info};
use uuid::Uuid;

use crate::db::DbHandle;
use crate::memory_index::{self, Embedded, Scope};
use crate::memory_kernel_service::{self as kernel, RecallResult};
use crate::memory_relations::S1Client;

pub mod eval;
#[cfg(test)]
mod tests;

pub const BANK_ROUTE: &str = "memory.route";
pub const BANK_IS_RELEVANT: &str = "memory.is_relevant";
pub const BANK_RANK: &str = "memory.rank";
/// Not an S1 bank: the would-be evidence set rows.
pub const BANK_SELECTED: &str = "memory.retrieve";
pub const LABEL_SOURCE: &str = "memory.answer_used";

/// Relations expansion follows (decision M1: updates / same / about_entity /
/// causes / part_of, plus the inverse direction of causes).
pub const FOLLOW: &[&str] = &["updates", "same", "about_entity", "causes", "caused_by", "part_of"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    Facts,
    Entities,
    Observations,
    Documents,
    Procedures,
    Notes,
}

impl Space {
    pub const ALL: &'static [Space] =
        &[Space::Facts, Space::Entities, Space::Observations, Space::Documents, Space::Procedures, Space::Notes];
    /// What the incumbent hybrid recall searches.
    pub const INCUMBENT: &'static [Space] = &[Space::Facts, Space::Entities, Space::Observations];

    pub fn as_str(self) -> &'static str {
        match self {
            Space::Facts => "facts",
            Space::Entities => "entities",
            Space::Observations => "observations",
            Space::Documents => "documents",
            Space::Procedures => "procedures",
            Space::Notes => "notes",
        }
    }
    pub fn parse(raw: &str) -> Option<Space> {
        let n = raw.trim().to_ascii_lowercase();
        Space::ALL.iter().copied().find(|s| s.as_str() == n || s.as_str().trim_end_matches('s') == n)
    }
    /// The space a recall item belongs to.
    pub fn of_item(item_type: &str) -> Option<Space> {
        match item_type {
            "fact" => Some(Space::Facts),
            "entity" => Some(Space::Entities),
            "observation" => Some(Space::Observations),
            "document" => Some(Space::Documents),
            "procedure" => Some(Space::Procedures),
            "note" => Some(Space::Notes),
            _ => None,
        }
    }
    fn index_type(self) -> &'static str {
        match self {
            Space::Facts => "fact",
            Space::Entities => "entity",
            Space::Observations => "observation",
            Space::Documents => "chunk",
            // V209 (WP-M1d): notes and procedures are in the shared index.
            Space::Procedures => "procedure",
            Space::Notes => "note",
        }
    }
}

/// Parse a ROUTE answer: `all`, one space, or a list (`facts, notes` / JSON
/// array, SUBSET-shaped). Facts are always searched (the base space).
pub fn parse_route(answer: &str) -> Vec<Space> {
    let a = answer.to_ascii_lowercase();
    if a.trim() == "all" {
        return Space::ALL.to_vec();
    }
    let mut out = vec![Space::Facts];
    for tok in a.split(|c: char| !c.is_ascii_alphabetic()) {
        if let Some(s) = Space::parse(tok) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrieveConfig {
    /// Evidence to return.
    pub k: usize,
    /// Anchors fetched per chosen space.
    pub anchors_per_space: usize,
    /// STOP once this many anchors passed IS_RELEVANT.
    pub enough_evidence: usize,
    /// IS_RELEVANT counts only at or above this confidence.
    pub min_relevance: f64,
    /// Loop bound: most IS_RELEVANT checks per query.
    pub max_checks: usize,
    /// Most items RANK scores (relevant anchors + neighbours).
    pub max_rank: usize,
    /// RANK score needed to be selected (high 1.0, medium 0.5, low 0).
    pub min_score: f64,
    /// Follow edges into superseded facts too.
    pub include_history: bool,
}

impl RetrieveConfig {
    pub fn with_k(k: usize) -> Self {
        Self {
            k,
            anchors_per_space: 5,
            enough_evidence: 3,
            min_relevance: 0.5,
            max_checks: 10,
            max_rank: 12,
            min_score: 0.5,
            include_history: false,
        }
    }
    /// `ALLTERNIT_MEMORY_EVIDENCE_COUNT`, `…_EVIDENCE_MIN_CONF`, `…_MAX_CHECKS`.
    pub fn from_env(k: usize) -> Self {
        let mut c = Self::with_k(k);
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok());
        if let Some(v) = num("ALLTERNIT_MEMORY_EVIDENCE_COUNT") {
            c.enough_evidence = (v as usize).max(1);
        }
        if let Some(v) = num("ALLTERNIT_MEMORY_EVIDENCE_MIN_CONF") {
            c.min_relevance = v;
        }
        if let Some(v) = num("ALLTERNIT_MEMORY_MAX_CHECKS") {
            c.max_checks = (v as usize).max(1);
        }
        c
    }
}

/// One S1 decision the pipeline made.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DecisionLog {
    pub bank: &'static str,
    pub decision_id: String,
    pub candidate: Option<String>,
    pub answer: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub item: RecallResult,
    /// `anchor:<space>` or `expansion:<relation>`.
    pub step: String,
    pub space: Option<Space>,
    pub score: f64,
    pub relevance_decision: Option<String>,
    pub rank_decision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RetrieveOutcome {
    pub spaces: Vec<Space>,
    pub route_decision: Option<String>,
    pub checks: usize,
    pub stopped_early: bool,
    pub expanded: usize,
    pub evidence: Vec<Evidence>,
    pub decisions: Vec<DecisionLog>,
    pub latency_ms: u128,
}

const ROUTE_INSTRUCTIONS: &str = "Which memory space should be searched to answer the question? \
facts = durable facts about the user; entities = people, places, projects; observations = past conversation turns; \
documents = saved documents; procedures = how-to steps the user taught; notes = the user's notes; all = more than one.";
const RELEVANT_INSTRUCTIONS: &str = "Does this memory help answer the question? \
relevant = it states something the answer needs; not_relevant = otherwise.";
const RANK_INSTRUCTIONS: &str = "How useful is this memory as evidence for answering the question? \
high = directly answers it; medium = useful context; low = not useful.";

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

const STOP: &[&str] = &[
    "the", "and", "for", "are", "was", "what", "who", "how", "why", "when", "where", "which", "does", "did",
    "you", "your", "with", "that", "this", "have", "has", "can", "about", "from", "into", "any", "all", "its",
    "our", "out", "but", "not", "his", "her", "they", "them", "she", "him", "had", "were", "will", "would",
];

/// Lowercase content tokens (≥3 chars, no stopwords).
pub fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.chars().count() >= 3 && !STOP.contains(&t.as_str()))
        .collect()
}

/// Weak "the answer used this memory" signal: the reply restates at least
/// half of the memory's content tokens (and at least two, or its only one).
pub fn answer_uses(answer: &str, content: &str) -> bool {
    let t = tokens(content);
    if t.is_empty() {
        return false;
    }
    let a = tokens(answer);
    let hit = t.iter().filter(|x| a.contains(*x)).count();
    let ratio = hit as f64 / t.len() as f64;
    ratio >= 0.5 && (hit >= 2 || t.len() == 1)
}

fn load_chunk(conn: &Connection, user_id: &str, id: &str) -> rusqlite::Result<Option<RecallResult>> {
    conn.query_row(
        "SELECT id, text, source_type, source_id, chunk_index, COALESCE(created_at, '') FROM memory_index_chunks
         WHERE id = ?1 AND scope = ?2",
        params![id, user_id],
        |r| {
            Ok(RecallResult {
                id: r.get(0)?,
                item_type: "document".into(),
                score: 0.0,
                content: r.get(1)?,
                metadata: json!({ "source_type": r.get::<_, String>(2)?, "source_id": r.get::<_, String>(3)?, "chunk_index": r.get::<_, i64>(4)? }),
                timestamp: r.get(5)?,
            })
        },
    )
    .optional()
}

/// One note or procedure, owned by `user_id` (and the agent, for procedures).
fn load_note_or_procedure(conn: &Connection, t: &str, user_id: &str, agent_id: Option<&str>, id: &str) -> rusqlite::Result<Option<RecallResult>> {
    let sql = if t == "note" {
        "SELECT id, title || ': ' || content, COALESCE(created_at, '') FROM memory_notes WHERE id = ?1 AND user_id = ?2 AND (?3 IS NULL OR 1)"
    } else {
        "SELECT id, name || ': ' || COALESCE(description, '') || ' ' || trigger_patterns || ' ' || steps, COALESCE(created_at, '')
         FROM procedural_memory WHERE id = ?1 AND user_id = ?2 AND (agent_id IS NULL OR ?3 IS NULL OR agent_id = ?3)"
    };
    conn.query_row(sql, params![id, user_id, agent_id], |r| {
        Ok(RecallResult { id: r.get(0)?, item_type: t.into(), score: 0.0, content: r.get(1)?, metadata: json!({}), timestamp: r.get(2)? })
    })
    .optional()
}

/// S0 anchors for one space, best first.
pub fn anchors(
    conn: &Connection,
    space: Space,
    user_id: &str,
    agent_id: Option<&str>,
    query: &str,
    qv: Option<&Embedded>,
    n: usize,
    include_history: bool,
) -> Result<Vec<RecallResult>, kernel::MemoryKernelError> {
    let t = space.index_type();
    let types = [t];
    let scope = Scope { scope: user_id, target_types: &types, only_ids: None };
    let v = qv.and_then(|e| e.vectors.first().map(|v| (e.model.as_str(), v.as_slice())));
    let hits = memory_index::hybrid_search(conn, &scope, query, v, n * 2)?;
    let mut out = vec![];
    for h in hits {
        let item = if t == "chunk" {
            load_chunk(conn, user_id, &h.target_id)?
        } else if t == "note" || t == "procedure" {
            load_note_or_procedure(conn, t, user_id, agent_id, &h.target_id)?
        } else {
            kernel::load_target(conn, t, &h.target_id, agent_id, include_history)?
        };
        if let Some(mut item) = item {
            item.score = h.score;
            out.push(item);
        }
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}

/// S0 graph neighbours of a node over the typed edges in [`FOLLOW`], either
/// direction: (relation, neighbour). Superseded facts are skipped unless
/// `include_history`.
pub fn neighbours(
    conn: &Connection,
    user_id: &str,
    agent_id: Option<&str>,
    node_id: &str,
    include_history: bool,
) -> Result<Vec<(String, RecallResult)>, kernel::MemoryKernelError> {
    let mut stmt = conn.prepare(
        "SELECT relation_type, source_entity_id, COALESCE(source_kind, 'fact'), target_entity_id, COALESCE(target_kind, 'fact')
         FROM memory_relationships
         WHERE user_id = ?1 AND relation_type IS NOT NULL AND (source_entity_id = ?2 OR target_entity_id = ?2)
         ORDER BY valid_from DESC, rowid DESC LIMIT 50",
    )?;
    let rows: Vec<(String, String, String, String, String)> = stmt
        .query_map(params![user_id, node_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let mut out = vec![];
    for (rel, src, sk, tgt, tk) in rows {
        if !FOLLOW.contains(&rel.as_str()) {
            continue;
        }
        let (other, kind) = if src == node_id { (tgt, tk) } else { (src, sk) };
        if other == node_id {
            continue;
        }
        if let Some(item) = kernel::load_target(conn, &kind, &other, agent_id, include_history)? {
            out.push((rel, item));
        }
    }
    Ok(out)
}

fn key(item: &RecallResult) -> (String, String) {
    (item.item_type.clone(), item.id.clone())
}

/// The retrieve pipeline. `run_id` keys the S1 requests (the recall-log id).
#[allow(clippy::too_many_arguments)]
pub async fn retrieve(
    client: &S1Client,
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    run_id: &str,
    query: &str,
    qv: Option<&Embedded>,
    cfg: &RetrieveConfig,
) -> Result<RetrieveOutcome, kernel::MemoryKernelError> {
    let started = Instant::now();
    let mut out = RetrieveOutcome::default();
    let q = clip(query, 1000);

    // 1. ROUTE.
    let route_labels: Vec<&str> = Space::ALL.iter().map(|s| s.as_str()).chain(["all"]).collect();
    out.spaces = match client.decide(BANK_ROUTE, "ROUTE", "memory.route", run_id, ROUTE_INSTRUCTIONS, &q, &route_labels).await {
        Some(a) if a.answer != "unknown" => {
            out.route_decision = Some(a.decision_id.clone());
            out.decisions.push(DecisionLog { bank: BANK_ROUTE, decision_id: a.decision_id, candidate: None, answer: a.answer.clone(), confidence: a.confidence });
            parse_route(&a.answer)
        }
        Some(a) => {
            out.decisions.push(DecisionLog { bank: BANK_ROUTE, decision_id: a.decision_id, candidate: None, answer: a.answer, confidence: a.confidence });
            Space::INCUMBENT.to_vec()
        }
        None => Space::INCUMBENT.to_vec(),
    };

    // 2. Anchors per space, interleaved by rank so the loop sees every
    //    chosen space early.
    let per_space: Vec<(Space, Vec<RecallResult>)> = {
        let conn = db.connect()?;
        out.spaces
            .iter()
            .map(|s| anchors(&conn, *s, user_id, agent_id, query, qv, cfg.anchors_per_space, cfg.include_history).map(|a| (*s, a)))
            .collect::<Result<_, _>>()?
    };
    let mut queue: Vec<(Space, RecallResult)> = vec![];
    for i in 0..cfg.anchors_per_space {
        for (s, list) in &per_space {
            if let Some(a) = list.get(i) {
                queue.push((*s, a.clone()));
            }
        }
    }

    // 3. Evidence loop: IS_RELEVANT per anchor; STOP when enough, else CONTINUE.
    let rel_labels = ["relevant", "not_relevant"];
    let mut relevant: Vec<Evidence> = vec![];
    for (space, item) in queue {
        if relevant.len() >= cfg.enough_evidence {
            out.stopped_early = true;
            break;
        }
        if out.checks >= cfg.max_checks {
            break;
        }
        out.checks += 1;
        let state = format!("question:\n{q}\n\nmemory ({}):\n{}", space.as_str(), clip(&item.content, 1500));
        let ans = client.decide(BANK_IS_RELEVANT, "GATE", "memory.is_relevant", run_id, RELEVANT_INSTRUCTIONS, &state, &rel_labels).await;
        let (keep, did) = match ans {
            Some(a) => {
                let keep = a.answer == "relevant" && a.confidence >= cfg.min_relevance;
                out.decisions.push(DecisionLog { bank: BANK_IS_RELEVANT, decision_id: a.decision_id.clone(), candidate: Some(item.id.clone()), answer: a.answer, confidence: a.confidence });
                (keep, Some(a.decision_id))
            }
            None => (true, None), // S0 fallback: keep in anchor order.
        };
        if keep {
            relevant.push(Evidence { score: item.score, item, step: format!("anchor:{}", space.as_str()), space: Some(space), relevance_decision: did, rank_decision: None });
        }
    }

    // 4. Expansion over typed edges (one hop).
    let mut seen: HashSet<(String, String)> = relevant.iter().map(|e| key(&e.item)).collect();
    let mut pool = relevant;
    {
        let conn = db.connect()?;
        let anchors_now: Vec<String> = pool.iter().filter(|e| matches!(e.item.item_type.as_str(), "fact" | "entity" | "observation")).map(|e| e.item.id.clone()).collect();
        for id in anchors_now {
            for (rel, item) in neighbours(&conn, user_id, agent_id, &id, cfg.include_history)? {
                if seen.insert(key(&item)) {
                    out.expanded += 1;
                    let space = Space::of_item(&item.item_type);
                    pool.push(Evidence { score: 0.0, item, step: format!("expansion:{rel}"), space, relevance_decision: None, rank_decision: None });
                }
            }
        }
    }
    pool.truncate(cfg.max_rank);

    // 5. RANK / SCORE.
    let rank_labels = ["high", "medium", "low"];
    let mut scored: Vec<(f64, f64, usize, Evidence)> = vec![];
    for (i, mut ev) in pool.into_iter().enumerate() {
        let state = format!("question:\n{q}\n\nmemory ({}):\n{}", ev.item.item_type, clip(&ev.item.content, 1500));
        let (score, conf) = match client.decide(BANK_RANK, "SCORE", "memory.rank", run_id, RANK_INSTRUCTIONS, &state, &rank_labels).await {
            Some(a) => {
                let s = match a.answer.as_str() {
                    "high" => 1.0,
                    "medium" => 0.5,
                    _ => 0.0,
                };
                out.decisions.push(DecisionLog { bank: BANK_RANK, decision_id: a.decision_id.clone(), candidate: Some(ev.item.id.clone()), answer: a.answer, confidence: a.confidence });
                ev.rank_decision = Some(a.decision_id);
                (s, a.confidence)
            }
            None => (1.0, 0.0), // S0 fallback: keep in pipeline order.
        };
        if score >= cfg.min_score {
            ev.score = score;
            scored.push((score, conf, i, ev));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)).then(a.2.cmp(&b.2))
    });
    out.evidence = scored.into_iter().take(cfg.k).map(|(_, _, _, e)| e).collect();
    out.latency_ms = started.elapsed().as_millis();
    Ok(out)
}

// ─── Shadow ─────────────────────────────────────────────────────────────────

fn shadow_slots() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(2))
}

/// What the shadow did, for logs/tests.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ShadowSummary {
    pub decisions: usize,
    pub would_be: Vec<String>,
    pub incumbent: Vec<String>,
    pub overlap: usize,
}

/// Run the pipeline in shadow for one incumbent recall. Never changes what
/// the caller returned; writes the decision ledger only. Skips (None) when
/// S1 is off or two shadows are already running.
#[allow(clippy::too_many_arguments)]
pub async fn shadow_recall(
    client: &S1Client,
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    recall_id: &str,
    query: &str,
    qv: Option<&Embedded>,
    incumbent: &[RecallResult],
    cfg: &RetrieveConfig,
) -> Option<ShadowSummary> {
    if !client.enabled || memory_index::fts_query(query).is_none() {
        return None;
    }
    let _slot = shadow_slots().try_acquire().ok()?;
    let outcome = match retrieve(client, db, user_id, agent_id, recall_id, query, qv, cfg).await {
        Ok(o) => o,
        Err(e) => {
            debug!(error = %e, "memory retrieve shadow failed");
            return None;
        }
    };
    if outcome.decisions.is_empty() {
        return None; // S1 never answered: nothing to learn from.
    }
    let returned: HashSet<&str> = incumbent.iter().map(|r| r.id.as_str()).collect();
    let inc = |id: &str| if returned.contains(id) { "returned" } else { "not_returned" };
    let conn = db.connect().ok()?;
    let insert = |bank: &str, did: &str, cand: Option<&str>, ans: &str, inc_label: &str| {
        let _ = conn.execute(
            "INSERT INTO memory_s1_decisions (id, user_id, bank, decision_id, observation_id, candidate_fact_id, s1_answer, incumbent_label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![format!("msd_{}", Uuid::new_v4().simple()), user_id, bank, did, recall_id, cand, ans, inc_label],
        );
    };
    let incumbent_spaces = Space::INCUMBENT.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",");
    for d in &outcome.decisions {
        let label = match &d.candidate {
            Some(c) => inc(c).to_string(),
            None => incumbent_spaces.clone(),
        };
        insert(d.bank, &d.decision_id, d.candidate.as_deref(), &d.answer, &label);
    }
    for e in &outcome.evidence {
        let did = e.rank_decision.as_deref().or(e.relevance_decision.as_deref()).or(outcome.route_decision.as_deref()).unwrap_or("");
        insert(BANK_SELECTED, did, Some(&e.item.id), &e.step, inc(&e.item.id));
    }
    let would_be: Vec<String> = outcome.evidence.iter().map(|e| e.item.id.clone()).collect();
    let summary = ShadowSummary {
        decisions: outcome.decisions.len(),
        overlap: would_be.iter().filter(|id| returned.contains(id.as_str())).count(),
        would_be,
        incumbent: incumbent.iter().map(|r| r.id.clone()).collect(),
    };
    info!(
        recall_id, decisions = summary.decisions, overlap = summary.overlap,
        would_be = ?summary.would_be, decision_ids = ?outcome.decisions.iter().map(|d| d.decision_id.as_str()).collect::<Vec<_>>(),
        latency_ms = outcome.latency_ms as u64, "memory retrieve shadow"
    );
    Some(summary)
}

// ─── Outcome labels from the answer ─────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LabelReport {
    pub recall_id: Option<String>,
    pub used: Vec<String>,
    pub unused: Vec<String>,
    pub not_shown: usize,
    pub outcomes_reported: usize,
}

/// Label the newest unlabelled shadow recall of this session (last 30 min)
/// from the assistant's reply. See module docs.
pub async fn label_from_answer(client: &S1Client, db: &DbHandle, user_id: &str, session_id: &str, answer: &str) -> LabelReport {
    let mut report = LabelReport::default();
    let Ok(conn) = db.connect() else { return report };
    let log: Option<(String, String)> = conn
        .query_row(
            "SELECT l.id, l.results FROM memory_recall_logs l
             WHERE l.user_id = ?1 AND l.session_id = ?2 AND l.created_at >= datetime('now', '-30 minutes')
               AND EXISTS (SELECT 1 FROM memory_s1_decisions d WHERE d.observation_id = l.id AND d.user_label IS NULL)
             ORDER BY l.created_at DESC, l.rowid DESC LIMIT 1",
            params![user_id, session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((recall_id, results)) = log else { return report };
    report.recall_id = Some(recall_id.clone());
    let shown: Vec<RecallResult> = serde_json::from_str(&results).unwrap_or_default();
    let used_of: HashMap<String, (bool, &str)> = shown.iter().map(|r| (r.id.clone(), (answer_uses(answer, &r.content), r.item_type.as_str()))).collect();
    for r in &shown {
        if used_of[&r.id].0 { report.used.push(r.id.clone()) } else { report.unused.push(r.id.clone()) }
    }
    let used_space = report.used.first().and_then(|id| Space::of_item(used_of[id].1)).map(|s| s.as_str());
    let rows: Vec<(String, String, String, Option<String>)> = conn
        .prepare("SELECT id, bank, decision_id, candidate_fact_id FROM memory_s1_decisions WHERE observation_id = ?1 AND user_label IS NULL")
        .and_then(|mut s| s.query_map(params![recall_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
        .unwrap_or_default();
    let mut to_report: Vec<(String, &'static str)> = vec![];
    for (row_id, bank, did, cand) in rows {
        let label: Option<String> = match cand.as_deref().map(|c| used_of.get(c).map(|u| u.0)) {
            Some(Some(true)) => {
                match bank.as_str() {
                    BANK_IS_RELEVANT => to_report.push((did, "relevant")),
                    BANK_RANK => to_report.push((did, "high")),
                    _ => {}
                }
                Some("used".into())
            }
            Some(Some(false)) => {
                match bank.as_str() {
                    BANK_IS_RELEVANT => to_report.push((did, "not_relevant")),
                    BANK_RANK => to_report.push((did, "low")),
                    _ => {}
                }
                Some("unused".into())
            }
            Some(None) => {
                report.not_shown += 1;
                Some("not_shown".into())
            }
            None => used_space.map(|s| {
                if bank == BANK_ROUTE {
                    to_report.push((did, s));
                }
                s.to_string()
            }),
        };
        let _ = conn.execute(
            "UPDATE memory_s1_decisions SET user_label = ?2 WHERE id = ?1",
            params![row_id, label.unwrap_or_else(|| "none_used".into())],
        );
    }
    drop(conn);
    for (did, truth) in to_report {
        if client.reporter.report(&did, truth, LABEL_SOURCE).await {
            report.outcomes_reported += 1;
        }
    }
    report
}

/// JSON view of an outcome for the eval harness / route debugging.
pub fn outcome_json(o: &RetrieveOutcome) -> Value {
    serde_json::to_value(o).unwrap_or(Value::Null)
}
