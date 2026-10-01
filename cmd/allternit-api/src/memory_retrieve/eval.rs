//! LoCoMo-style long-conversation QA eval for the retrieve path: the
//! incumbent hybrid recall vs the S1 pipeline ([`super::retrieve`]), scored
//! on recall@k, evidence precision and latency.
//!
//! Fixture format (JSON; `fixtures/tiny_locomo.json` is the in-repo one):
//!
//! ```json
//! { "name": "…", "conversations": [ {
//!     "id": "conv-1",
//!     "turns": [ { "id": "D1:1", "speaker": "Ana", "text": "…", "session": "1" } ],
//!     "facts": [ { "id": "F1", "text": "…", "edges": [ { "relation": "updates", "to": "F0" } ] } ],
//!     "qa":    [ { "question": "…", "evidence": ["D1:1", "F1"], "answer": "…" } ] } ] }
//! ```
//!
//! Turns become observations, `facts` (optional) become facts, `edges` become
//! typed relations (an `updates`/`contradicts` edge supersedes its target,
//! exactly as the write path does). Each conversation is its own user, so
//! conversations never see each other. Gold `evidence` ids are turn or fact
//! ids. Public LoCoMo (`locomo10.json`) converts with
//! `tools/memory-eval/locomo_to_fixture.py` (turn `dia_id`s are the evidence
//! ids already); it is not vendored here (license: check the upstream repo
//! before redistributing). Run: `tools/memory-eval/run.sh <fixture.json>`.
use std::collections::HashSet;
use std::time::Instant;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{retrieve, RetrieveConfig};
use crate::db::DbHandle;
use crate::memory_index::{self, EmbedClient, InputType};
use crate::memory_kernel_service as kernel;
use crate::memory_relations::{self as rel, NodeKind, RelationType, S1Client};

#[derive(Debug, Clone, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub conversations: Vec<Conversation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub facts: Vec<FactSeed>,
    pub qa: Vec<Qa>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Turn {
    pub id: String,
    #[serde(default)]
    pub speaker: Option<String>,
    pub text: String,
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactSeed {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub edges: Vec<EdgeSeed>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeSeed {
    pub relation: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Qa {
    pub question: String,
    pub evidence: Vec<String>,
    #[serde(default)]
    pub answer: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ArmScore {
    pub recall_at_k: f64,
    pub evidence_precision: f64,
    pub latency_ms_mean: f64,
    pub latency_ms_p50: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuestionResult {
    pub conversation: String,
    pub question: String,
    pub gold: Vec<String>,
    pub incumbent: Vec<String>,
    pub pipeline: Vec<String>,
    pub pipeline_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub fixture: String,
    pub k: usize,
    pub questions: usize,
    pub s1_enabled: bool,
    pub embedding_model: String,
    pub incumbent: ArmScore,
    pub pipeline: ArmScore,
    pub per_question: Vec<QuestionResult>,
}

fn user_of(conv: &str) -> String {
    format!("eval:{conv}")
}

/// Load one conversation into the store. Ids are kept as given (prefixed
/// with the conversation id so conversations don't collide).
pub fn load_conversation(db: &DbHandle, conv: &Conversation) -> Result<(), kernel::MemoryKernelError> {
    let user = user_of(&conv.id);
    let conn = db.connect()?;
    let id = |raw: &str| format!("{}::{raw}", conv.id);
    for t in &conv.turns {
        let text = match &t.speaker {
            Some(s) => format!("{s}: {}", t.text),
            None => t.text.clone(),
        };
        conn.execute(
            "INSERT INTO memory_observations (id, user_id, session_id, kind, content, source) VALUES (?1, ?2, ?3, 'turn', ?4, 'eval')",
            params![id(&t.id), user, t.session, text],
        )?;
        kernel::store_embedding(db, &user, "observation", &id(&t.id), &text)?;
    }
    for f in &conv.facts {
        conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, ?2, ?3, 0.9)", params![id(&f.id), user, f.text])?;
        kernel::store_embedding(db, &user, "fact", &id(&f.id), &f.text)?;
    }
    let is_fact: HashSet<&str> = conv.facts.iter().map(|f| f.id.as_str()).collect();
    let kind = |raw: &str| if is_fact.contains(raw) { NodeKind::Fact } else { NodeKind::Observation };
    for f in &conv.facts {
        for e in &f.edges {
            let Some(r) = RelationType::parse(&e.relation) else { continue };
            rel::write_relation(&conn, &user, (NodeKind::Fact, &id(&f.id)), r, (kind(&e.to), &id(&e.to)), 0.9, rel::ORIGIN_INCUMBENT, None)?;
        }
    }
    Ok(())
}

fn strip(conv: &str, ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let p = format!("{conv}::");
    ids.into_iter().map(|i| i.strip_prefix(&p).map(str::to_string).unwrap_or(i)).collect()
}

fn score(rows: &[(Vec<String>, Vec<String>, f64)]) -> ArmScore {
    if rows.is_empty() {
        return ArmScore::default();
    }
    let n = rows.len() as f64;
    let (mut rec, mut prec) = (0.0, 0.0);
    let mut lat: Vec<f64> = vec![];
    for (gold, got, ms) in rows {
        let g: HashSet<&String> = gold.iter().collect();
        let hit = got.iter().filter(|x| g.contains(x)).count() as f64;
        rec += if g.is_empty() { 1.0 } else { hit / g.len() as f64 };
        prec += if got.is_empty() { 0.0 } else { hit / got.len() as f64 };
        lat.push(*ms);
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ArmScore {
        recall_at_k: rec / n,
        evidence_precision: prec / n,
        latency_ms_mean: lat.iter().sum::<f64>() / n,
        latency_ms_p50: lat[lat.len() / 2],
    }
}

/// Run the eval on a fresh store. `embed` embeds queries (hash fallback when
/// it has no URL; with a URL the stored targets are re-indexed first).
pub async fn run_eval(db: &DbHandle, fixture: &Fixture, s1: &S1Client, embed: &EmbedClient, k: usize) -> Result<EvalReport, kernel::MemoryKernelError> {
    for c in &fixture.conversations {
        load_conversation(db, c)?;
    }
    if embed.is_enabled() {
        while let Ok(r) = memory_index::index_pending(db, embed, 256).await {
            if r.embedded == 0 {
                break;
            }
        }
    }
    let cfg = RetrieveConfig::with_k(k);
    let (mut inc_rows, mut pipe_rows, mut per_question) = (vec![], vec![], vec![]);
    let mut model = String::new();
    for c in &fixture.conversations {
        let user = user_of(&c.id);
        for (i, qa) in c.qa.iter().enumerate() {
            let q = embed.embed_or_hash(&[qa.question.clone()], InputType::Query).await;
            model = q.model.clone();
            let t = Instant::now();
            let (inc, _) = kernel::recall_logged(db, &user, None, None, &qa.question, Some(&q), k)?;
            let inc_ms = t.elapsed().as_secs_f64() * 1000.0;
            let out = retrieve(s1, db, &user, None, &format!("eval:{}:{i}", c.id), &qa.question, Some(&q), &cfg).await?;
            let inc_ids = strip(&c.id, inc.into_iter().map(|r| r.id));
            let pipe_ids = strip(&c.id, out.evidence.iter().map(|e| e.item.id.clone()));
            inc_rows.push((qa.evidence.clone(), inc_ids.clone(), inc_ms));
            pipe_rows.push((qa.evidence.clone(), pipe_ids.clone(), out.latency_ms as f64));
            per_question.push(QuestionResult {
                conversation: c.id.clone(),
                question: qa.question.clone(),
                gold: qa.evidence.clone(),
                incumbent: inc_ids,
                pipeline: pipe_ids,
                pipeline_steps: out.evidence.iter().map(|e| e.step.clone()).collect(),
            });
        }
    }
    Ok(EvalReport {
        fixture: fixture.name.clone(),
        k,
        questions: per_question.len(),
        s1_enabled: s1.enabled,
        embedding_model: model,
        incumbent: score(&inc_rows),
        pipeline: score(&pipe_rows),
        per_question,
    })
}
