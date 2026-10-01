//! O9 — S1-gated semantic cache, SHADOW ONLY (WP-K1).
//!
//! Flow, per eligible internal completion (after the real answer is in, off
//! the caller's critical path):
//! 1. embed the query (the M1a embeddings client, `memory_index`), find the
//!    nearest cached answer of the same call type and model class;
//! 2. above the candidate similarity floor, ask S1 a CONFIDENCE_GATE
//!    question through `POST /v1/decision` (GATE, IS_DUPLICATE: "does this
//!    cached answer answer this query");
//! 3. log would-have-served (S1 says yes AND threshold_action is AUTO or the
//!    confidence clears τ) — it NEVER serves in this build;
//! 4. label the decision from the real answer: S0 exact match, else embedding
//!    similarity of real vs cached answer ≥ the label floor → "true", else
//!    "false"; report it to `POST /v1/decision/outcome`;
//! 5. insert (query, real answer) as a future candidate.
//!
//! Eligibility is [`crate::completion_cache::CachePolicy::semantic_eligible`]:
//! FAQ/doc-type, tool-free, non-personal, side-effect-free call types only.
//! Serving (leaving shadow) requires the bank to pass Q26 first; there is no
//! serve path here on purpose.
//!
//! Env: `ALLTERNIT_SEMANTIC_CACHE=off` disables; anything else = shadow.
//! The S1 runtime URL/token come from the shared outcome reporter env
//! (`ALLTERNIT_S1_URL` / `SYSTEM_ONE_URL`, `SYSTEM_ONE_TOKEN`).

use futures::future::BoxFuture;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::completion_cache::CallType;

/// Decision bank for the semantic-cache gate (shadow ledger join).
pub const BANK_ID: &str = "semantic_cache.is_duplicate";
pub const PRIMITIVE_ID: &str = "dec.confidence_gate.is_duplicate";

/// Transport to the decision runtime (injectable for tests).
pub trait DecisionTransport: Send + Sync {
    /// POST `path` (e.g. `/v1/decision`) with a JSON body; JSON reply.
    fn post(&self, path: &str, body: Value) -> BoxFuture<'static, Result<Value, String>>;
}

/// Embeddings source (injectable for tests). Vectors must be L2-normalized.
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: Vec<String>) -> BoxFuture<'static, Option<(String, Vec<Vec<f32>>)>>;
}

/// HTTP transport against the S1 runtime.
pub struct HttpTransport {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl HttpTransport {
    pub fn from_env() -> Self {
        let r = allternit_commrails::kernel::s1_outcome::OutcomeReporter::from_env();
        let http = reqwest::Client::builder()
            .timeout(r.timeout.max(Duration::from_secs(2)))
            .build()
            .unwrap_or_default();
        Self { base_url: r.base_url.trim_end_matches('/').to_string(), token: r.token, http }
    }
}

impl DecisionTransport for HttpTransport {
    fn post(&self, path: &str, body: Value) -> BoxFuture<'static, Result<Value, String>> {
        let mut rq = self.http.post(format!("{}{}", self.base_url, path)).json(&body);
        if let Some(t) = &self.token {
            rq = rq.bearer_auth(t);
        }
        Box::pin(async move {
            let r = rq.send().await.map_err(|e| e.to_string())?;
            if !r.status().is_success() {
                return Err(format!("decision runtime returned {}", r.status()));
            }
            r.json::<Value>().await.map_err(|e| e.to_string())
        })
    }
}

/// The M1a embeddings client (remote endpoint, hash fallback).
pub struct MemoryIndexEmbedder;

impl Embedder for MemoryIndexEmbedder {
    fn embed(&self, texts: Vec<String>) -> BoxFuture<'static, Option<(String, Vec<Vec<f32>>)>> {
        Box::pin(async move {
            let e = crate::memory_index::global()
                .embed_or_hash(&texts, crate::memory_index::InputType::Query)
                .await;
            (e.vectors.len() == texts.len()).then_some((e.model, e.vectors))
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticConfig {
    pub enabled: bool,
    /// Cosine floor for a nearest neighbour to become a candidate.
    pub candidate_min_sim: f32,
    /// τ: S1 confidence (for "true") at which a hit would have been served.
    pub tau: f64,
    /// Cosine floor at which the real and cached answers count as the same.
    pub label_min_sim: f32,
    pub max_entries: usize,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self { enabled: true, candidate_min_sim: 0.85, tau: 0.9, label_min_sim: 0.92, max_entries: 2048 }
    }
}

impl SemanticConfig {
    pub fn from_env() -> Self {
        let off = matches!(
            std::env::var("ALLTERNIT_SEMANTIC_CACHE").ok().as_deref().map(str::trim),
            Some("off") | Some("0") | Some("false")
        );
        Self { enabled: !off, ..Self::default() }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    call_type: CallType,
    model_class: String,
    embed_model: String,
    vector: Vec<f32>,
    query: String,
    answer: String,
}

/// Outcome of one shadow pass (returned for tests and logs).
#[derive(Debug, Clone, PartialEq)]
pub enum ShadowOutcome {
    /// Not eligible / disabled: nothing happened.
    Skipped,
    /// No neighbour above the candidate floor; the answer was indexed.
    NoCandidate,
    /// S1 was asked; `would_serve` is what serving mode would have done;
    /// `label` is the S0 truth reported for the decision.
    Gated { would_serve: bool, label: bool, decision_id: Option<String>, similarity: f32 },
    /// The decision runtime was unreachable/bad; nothing labelled.
    GateError(String),
}

pub struct SemanticCache {
    config: SemanticConfig,
    embedder: Arc<dyn Embedder>,
    transport: Arc<dyn DecisionTransport>,
    entries: Mutex<VecDeque<Entry>>,
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Model class: provider + model family (the trailing date/revision suffix
/// is dropped), so answers are only compared within one class.
pub fn model_class(model: &str) -> String {
    let m = model.trim().to_ascii_lowercase();
    let parts: Vec<&str> = m.split('-').collect();
    let keep = parts
        .iter()
        .rposition(|p| !(p.len() >= 6 && p.chars().all(|c| c.is_ascii_digit())))
        .map(|i| i + 1)
        .unwrap_or(parts.len());
    parts[..keep].join("-")
}

/// The DecisionRequestV1 body for the IS_DUPLICATE confidence gate.
pub fn gate_request(call_type: CallType, query: &str, cached_answer: &str, subject: &str) -> Value {
    let state = format!("QUERY:\n{query}\n\nCACHED ANSWER:\n{cached_answer}");
    json!({
        "state": state,
        "reversible": true,
        "backend": "auto",
        "request": {
            "envelope": {
                "abi_version": "1.0.0",
                "schema_id": "allternit.kernel.DecisionRequestV1",
                "schema_version": "1.0.0",
                "run_id": subject,
                "producer": { "component_id": "allternit-api.semantic_cache", "component_version": env!("CARGO_PKG_VERSION"), "kind": "RUNTIME" },
            },
            "operation": "GATE",
            "state_projection_ref": format!("semcache:{}:{subject}", call_type.as_str()),
            "instructions": "IS_DUPLICATE: does the cached answer fully answer this query?",
            "decision_bank_id": BANK_ID,
            "calibration_domain": PRIMITIVE_ID,
            "extensions": { "x-primitive_id": PRIMITIVE_ID, "x-motif": "CONFIDENCE_GATE", "x-subject_ref": subject },
        }
    })
}

/// Read a GATE verdict: (says_yes, confidence_for_yes, threshold_action, decision_id).
pub fn read_gate(result: &Value) -> (bool, f64, String, Option<String>) {
    let answer = &result["answer"];
    let yes = match answer {
        Value::Bool(b) => *b,
        Value::String(s) => s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("yes"),
        Value::Object(o) => o
            .get("value")
            .map(|v| v.as_bool().unwrap_or(false) || v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("true")))
            .unwrap_or(false),
        _ => false,
    };
    let conf = result["confidence"].as_f64().unwrap_or(0.0);
    // Confidence is for the top answer; expose P(yes).
    let p_yes = result["probabilities"]["true"].as_f64().unwrap_or(if yes { conf } else { 1.0 - conf });
    let action = result["threshold_action"].as_str().unwrap_or("").to_string();
    let id = result["extensions"]["x-decision_id"].as_str().map(str::to_string);
    (yes, p_yes, action, id)
}

impl SemanticCache {
    pub fn new(config: SemanticConfig, embedder: Arc<dyn Embedder>, transport: Arc<dyn DecisionTransport>) -> Self {
        Self { config, embedder, transport, entries: Mutex::new(VecDeque::new()) }
    }

    pub fn global() -> &'static SemanticCache {
        static G: OnceLock<SemanticCache> = OnceLock::new();
        G.get_or_init(|| {
            SemanticCache::new(SemanticConfig::from_env(), Arc::new(MemoryIndexEmbedder), Arc::new(HttpTransport::from_env()))
        })
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn nearest(&self, call_type: CallType, class: &str, embed_model: &str, v: &[f32]) -> Option<(Entry, f32)> {
        let entries = self.entries.lock().ok()?;
        entries
            .iter()
            .filter(|e| e.call_type == call_type && e.model_class == class && e.embed_model == embed_model)
            .map(|e| (e, cosine(&e.vector, v)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(e, s)| (e.clone(), s))
    }

    fn insert(&self, e: Entry) {
        if let Ok(mut entries) = self.entries.lock() {
            // One entry per exact query: replace instead of duplicating.
            entries.retain(|x| !(x.call_type == e.call_type && x.model_class == e.model_class && x.query == e.query));
            while entries.len() >= self.config.max_entries.max(1) {
                entries.pop_front();
            }
            entries.push_back(e);
        }
    }

    /// One shadow pass for a completed real answer. Never serves.
    pub async fn shadow(&self, call_type: CallType, model: &str, query: &str, real_answer: &str) -> ShadowOutcome {
        if !self.config.enabled || !call_type.policy().semantic_eligible() || real_answer.trim().is_empty() {
            return ShadowOutcome::Skipped;
        }
        let ct = call_type.as_str();
        let class = model_class(model);
        let Some((embed_model, vectors)) = self.embedder.embed(vec![query.to_string()]).await else {
            return ShadowOutcome::Skipped;
        };
        let Some(qv) = vectors.into_iter().next() else { return ShadowOutcome::Skipped };
        let candidate = self.nearest(call_type, &class, &embed_model, &qv);
        let entry = Entry {
            call_type,
            model_class: class,
            embed_model: embed_model.clone(),
            vector: qv,
            query: query.to_string(),
            answer: real_answer.to_string(),
        };
        let Some((cand, similarity)) = candidate.filter(|(_, s)| *s >= self.config.candidate_min_sim) else {
            self.insert(entry);
            return ShadowOutcome::NoCandidate;
        };
        crate::metrics::inc_completion_cache_event("internal", ct, "sem_candidate");
        let subject = format!("semcache-{}", &sha(&format!("{ct}\0{query}\0{}", cand.query))[..24]);
        let result = self.transport.post("/v1/decision", gate_request(call_type, query, &cand.answer, &subject)).await;
        let outcome = match result {
            Err(err) => ShadowOutcome::GateError(err),
            Ok(result) => {
                let (yes, p_yes, action, decision_id) = read_gate(&result);
                let would_serve = yes && (action.eq_ignore_ascii_case("AUTO") || p_yes >= self.config.tau);
                crate::metrics::inc_completion_cache_event("internal", ct, if would_serve { "would_serve" } else { "would_not_serve" });
                let label = self.label(&cand.answer, real_answer, &embed_model).await;
                crate::metrics::inc_completion_cache_event("internal", ct, if label { "shadow_same" } else { "shadow_different" });
                let source = if cand.answer.trim() == real_answer.trim() { "s0:exact_match" } else { "s0:answer_embedding_similarity" };
                let mut report = json!({ "truth": if label { "true" } else { "false" }, "source": source, "question_id": PRIMITIVE_ID });
                match &decision_id {
                    Some(id) => report["decision_id"] = json!(id),
                    None => report["subject_ref"] = json!(subject),
                }
                if let Err(err) = self.transport.post("/v1/decision/outcome", report).await {
                    tracing::debug!(error = %err, "semantic cache: outcome report failed");
                }
                tracing::info!(call_type = ct, similarity, would_serve, label, p_yes, action = %action,
                    "semantic cache shadow: would-have-served vs real answer");
                ShadowOutcome::Gated { would_serve, label, decision_id, similarity }
            }
        };
        self.insert(entry);
        outcome
    }

    /// S0 label: exact match, else embedding similarity of the two answers.
    async fn label(&self, cached: &str, real: &str, embed_model: &str) -> bool {
        if cached.trim() == real.trim() {
            return true;
        }
        match self.embedder.embed(vec![cached.to_string(), real.to_string()]).await {
            Some((m, v)) if m == embed_model && v.len() == 2 => cosine(&v[0], &v[1]) >= self.config.label_min_sim,
            _ => false,
        }
    }

    /// Fire-and-forget shadow pass on the runtime (no-op when ineligible).
    pub fn spawn_shadow(call_type: CallType, model: String, query: String, real_answer: String) {
        if !call_type.policy().semantic_eligible() {
            return;
        }
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                let _ = SemanticCache::global().shadow(call_type, &model, &query, &real_answer).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bag-of-words embedder: deterministic, normalized.
    struct WordEmbedder;
    impl Embedder for WordEmbedder {
        fn embed(&self, texts: Vec<String>) -> BoxFuture<'static, Option<(String, Vec<Vec<f32>>)>> {
            Box::pin(async move {
                let v = texts
                    .iter()
                    .map(|t| {
                        let mut v = vec![0f32; 64];
                        for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
                            v[(sha(w).as_bytes()[0] as usize) % 64] += 1.0;
                        }
                        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                        v.iter().map(|x| x / n).collect()
                    })
                    .collect();
                Some(("words".to_string(), v))
            })
        }
    }

    /// Records every POST; answers /v1/decision with a canned result.
    struct MockTransport {
        reply: Value,
        calls: Mutex<Vec<(String, Value)>>,
    }
    impl MockTransport {
        fn new(reply: Value) -> Arc<Self> {
            Arc::new(Self { reply, calls: Mutex::new(vec![]) })
        }
        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl DecisionTransport for MockTransport {
        fn post(&self, path: &str, body: Value) -> BoxFuture<'static, Result<Value, String>> {
            self.calls.lock().unwrap().push((path.to_string(), body));
            let r = if path == "/v1/decision" { self.reply.clone() } else { json!({}) };
            Box::pin(async move { Ok(r) })
        }
    }

    fn cache(t: Arc<MockTransport>) -> SemanticCache {
        SemanticCache::new(SemanticConfig::default(), Arc::new(WordEmbedder), t)
    }

    fn yes(conf: f64, action: &str) -> Value {
        json!({ "operation": "GATE", "answer": "true", "confidence": conf, "threshold_action": action,
                "extensions": { "x-decision_id": "dec-1" } })
    }

    const Q: &str = "create a lesson on rust ownership and borrowing basics";

    #[tokio::test]
    async fn shadow_logs_would_serve_and_reports_same_label() {
        let t = MockTransport::new(yes(0.95, "REVIEW"));
        let c = cache(t.clone());
        assert_eq!(c.shadow(CallType::AlabsLesson, "p/m", Q, "LESSON A").await, ShadowOutcome::NoCandidate);
        let out = c.shadow(CallType::AlabsLesson, "p/m", Q, "LESSON A").await;
        assert!(matches!(out, ShadowOutcome::Gated { would_serve: true, label: true, .. }), "{out:?}");
        let calls = t.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "/v1/decision");
        let body = &calls[0].1;
        assert_eq!(body["backend"], "auto");
        assert_eq!(body["reversible"], true);
        assert_eq!(body["request"]["operation"], "GATE");
        assert_eq!(body["request"]["envelope"]["schema_id"], "allternit.kernel.DecisionRequestV1");
        assert_eq!(body["request"]["decision_bank_id"], BANK_ID);
        assert!(body["state"].as_str().unwrap().contains("LESSON A"));
        assert_eq!(calls[1].0, "/v1/decision/outcome");
        assert_eq!(calls[1].1["decision_id"], "dec-1");
        assert_eq!(calls[1].1["truth"], "true");
        assert_eq!(calls[1].1["source"], "s0:exact_match");
    }

    #[tokio::test]
    async fn different_real_answer_is_labelled_false() {
        let t = MockTransport::new(yes(0.97, "AUTO"));
        let c = cache(t.clone());
        c.shadow(CallType::AlabsLesson, "p/m", Q, "alpha beta gamma delta").await;
        let out = c.shadow(CallType::AlabsLesson, "p/m", Q, "zulu yankee xray whiskey victor").await;
        assert!(matches!(out, ShadowOutcome::Gated { would_serve: true, label: false, .. }), "{out:?}");
        assert_eq!(t.calls()[1].1["truth"], "false");
    }

    #[tokio::test]
    async fn low_confidence_or_no_is_not_served() {
        let t = MockTransport::new(yes(0.6, "REVIEW"));
        let c = cache(t.clone());
        c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await;
        assert!(matches!(c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await, ShadowOutcome::Gated { would_serve: false, .. }));
        let t = MockTransport::new(json!({ "answer": "false", "confidence": 0.99, "threshold_action": "AUTO" }));
        let c = cache(t.clone());
        c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await;
        let out = c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await;
        assert!(matches!(out, ShadowOutcome::Gated { would_serve: false, .. }), "{out:?}");
        // No decision id → the outcome joins by subject_ref.
        assert!(t.calls()[1].1["subject_ref"].as_str().unwrap().starts_with("semcache-"));
    }

    #[tokio::test]
    async fn excluded_call_types_never_touch_s1_or_the_store() {
        let t = MockTransport::new(yes(0.99, "AUTO"));
        let c = cache(t.clone());
        for ct in [CallType::MemoryExtraction, CallType::MemoryCuration, CallType::AgencyNode,
                   CallType::CoworkTeam, CallType::CoordinatorPlan, CallType::GatewayChat, CallType::Unclassified] {
            assert_eq!(c.shadow(ct, "p/m", Q, "A").await, ShadowOutcome::Skipped, "{ct:?}");
            assert_eq!(c.shadow(ct, "p/m", Q, "A").await, ShadowOutcome::Skipped, "{ct:?}");
        }
        assert!(t.calls().is_empty());
        assert!(c.is_empty());
    }

    #[tokio::test]
    async fn neighbours_only_within_call_type_and_model_class() {
        let t = MockTransport::new(yes(0.99, "AUTO"));
        let c = cache(t.clone());
        c.shadow(CallType::AlabsLesson, "prov/model-a", Q, "A").await;
        assert_eq!(c.shadow(CallType::AlabsLesson, "prov/model-b", Q, "A").await, ShadowOutcome::NoCandidate);
        // Same family, different dated revision → same class.
        c.shadow(CallType::AlabsLesson, "prov/fam-20250101", Q, "A").await;
        assert!(matches!(c.shadow(CallType::AlabsLesson, "prov/fam-20260101", Q, "A").await, ShadowOutcome::Gated { .. }));
        // Unrelated query → no candidate.
        assert_eq!(c.shadow(CallType::AlabsLesson, "prov/model-a", "kubernetes pod networking quiz", "B").await, ShadowOutcome::NoCandidate);
    }

    #[tokio::test]
    async fn disabled_and_gate_errors_are_safe() {
        let t = MockTransport::new(yes(0.99, "AUTO"));
        let c = SemanticCache::new(SemanticConfig { enabled: false, ..Default::default() }, Arc::new(WordEmbedder), t.clone());
        assert_eq!(c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await, ShadowOutcome::Skipped);
        struct Down;
        impl DecisionTransport for Down {
            fn post(&self, _: &str, _: Value) -> BoxFuture<'static, Result<Value, String>> {
                Box::pin(async { Err("connection refused".to_string()) })
            }
        }
        let c = SemanticCache::new(SemanticConfig::default(), Arc::new(WordEmbedder), Arc::new(Down));
        c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await;
        assert!(matches!(c.shadow(CallType::AlabsLesson, "p/m", Q, "A").await, ShadowOutcome::GateError(_)));
    }

    #[test]
    fn model_class_drops_dated_revisions() {
        assert_eq!(model_class("anthropic/x-sonnet-20250514"), "anthropic/x-sonnet");
        assert_eq!(model_class("p/m"), "p/m");
    }

    #[test]
    fn read_gate_accepts_answer_shapes() {
        assert!(read_gate(&json!({"answer": true, "confidence": 0.9})).0);
        assert!(read_gate(&json!({"answer": {"value": "true"}, "confidence": 0.9})).0);
        let (y, p, _, _) = read_gate(&json!({"answer": "false", "confidence": 0.8}));
        assert!(!y && (p - 0.2).abs() < 1e-9);
        let (_, p, _, _) = read_gate(&json!({"answer": "true", "confidence": 0.8, "probabilities": {"true": 0.7}}));
        assert!((p - 0.7).abs() < 1e-9);
    }
}
