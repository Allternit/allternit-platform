//! `head.embed`: a CPU-trained embedding head (Agency Descent B2/B3).
//!
//! The state (context + question) and every option text are embedded with
//! the shared embedding model (EmbeddingGemma 2 at `ALLTERNIT_EMBED_URL`;
//! the deterministic hash embedding when it is down). A pairwise scorer
//! rates each (state, option) pair and a softmax over the decision's options
//! gives the probabilities:
//!
//! - **Features** of a pair (s, o), both unit vectors of dimension d:
//!   `[s ⊙ o, |s − o|, s · o]`, 2d + 1 values. `s ⊙ o` lets the head weight
//!   the dimensions where state and option agree, `|s − o|` where they differ,
//!   and `s · o` (the cosine) is the zero-shot retrieval score it starts from.
//! - **Model**: a linear score `w · f(s, o)`, trained with the listwise
//!   softmax cross-entropy over each decision's options (multinomial logistic
//!   regression with shared weights), L2-regularized. Features are
//!   standardized for training; the scale is folded into the stored weights
//!   and the mean cancels in the softmax, so serving is one dot product per
//!   option. 768-dim vectors give 1537 weights (6 KB).
//! - **Deterministic**: zero init, full-batch Adam with fixed epochs and
//!   learning rate, examples in content-hash order, sequential f64 gradient
//!   sums. No RNG. The same dataset and embeddings give bit-identical weights
//!   (`weights_sha256` in the metrics).
//!
//! Why in Rust, in-process: there is no GPU, the model is tiny and training a
//! few thousand examples is seconds of CPU; it needs no new runtime or
//! dependency, uses the same embedding client and the same SQLite store, and
//! the arithmetic order is fixed by this file alone.
//!
//! Model mixing is refused: an artifact records the embedding model id and
//! dimension it was trained on, and the head does not answer for vectors
//! from any other model (for example the hash fallback while the embedding
//! server is down, for a head trained on EmbeddingGemma 2).

use super::flywheel::{Head, HeadAnswer, HeadInput, Vectors};
use super::training;
use crate::memory_index::{EmbedClient, InputType};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const NAME: &str = "head.embed";

/// The embedding of a decision's state: question last, after the context.
pub fn state_text(context: &str, question: Option<&str>) -> String {
    match question.filter(|q| !q.trim().is_empty()) {
        Some(q) => format!("{context}\n\nQuestion: {q}"),
        None => context.to_string(),
    }
}

/// Embed one decision: the state as a query, the options as documents.
/// `None` when the two calls came back from different models (the server
/// went down between them): such vectors must not be scored together.
pub async fn embed_decision(client: &EmbedClient, context: &str, question: Option<&str>, texts: &[&str]) -> Option<Vectors> {
    let state = client.embed_or_hash(&[state_text(context, question)], InputType::Query).await;
    let options = client.embed_or_hash(&texts.iter().map(|t| t.to_string()).collect::<Vec<_>>(), InputType::Document).await;
    if state.model != options.model || state.dim != options.dim || options.vectors.len() != texts.len() {
        return None;
    }
    Some(Vectors { model: state.model, state: state.vectors.into_iter().next()?, options: options.vectors })
}

// ── model ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TrainConfig {
    pub epochs: usize,
    pub lr: f32,
    pub l2: f32,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self { epochs: 150, lr: 0.05, l2: 1e-3 }
    }
}

/// One training decision: indexes into a vector table.
#[derive(Clone, Debug)]
pub struct Example {
    pub state: usize,
    pub options: Vec<usize>,
    pub label: usize,
}

pub fn n_features(dim: usize) -> usize {
    2 * dim + 1
}

/// `[s ⊙ o, |s − o|, s · o]` into `out` (length `2 * dim + 1`).
pub fn features(s: &[f32], o: &[f32], out: &mut [f32]) {
    let d = s.len();
    let mut dot = 0.0f32;
    for i in 0..d {
        let p = s[i] * o[i];
        out[i] = p;
        out[d + i] = (s[i] - o[i]).abs();
        dot += p;
    }
    out[2 * d] = dot;
}

fn dot(w: &[f32], x: &[f32]) -> f64 {
    let mut acc = 0.0f64;
    for i in 0..w.len() {
        acc += (w[i] * x[i]) as f64;
    }
    acc
}

pub fn softmax(scores: &[f64]) -> Vec<f64> {
    let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exp: Vec<f64> = scores.iter().map(|s| (s - max).exp()).collect();
    let sum: f64 = exp.iter().sum();
    exp.iter().map(|e| e / sum).collect()
}

/// Probabilities over the options of one decision.
pub fn predict_probs(w: &[f32], state: &[f32], options: &[&[f32]]) -> Vec<f64> {
    let mut buf = vec![0.0f32; n_features(state.len())];
    let scores: Vec<f64> = options
        .iter()
        .map(|o| {
            features(state, o, &mut buf);
            dot(w, &buf)
        })
        .collect();
    softmax(&scores)
}

/// Fit the scorer. `vecs` holds every state and option vector (all of
/// dimension `dim`); `train` is in a fixed order. Returns the folded weights.
pub fn fit(vecs: &[Vec<f32>], train: &[Example], dim: usize, cfg: &TrainConfig) -> Vec<f32> {
    let f = n_features(dim);
    let mut buf = vec![0.0f32; f];
    // Feature scale over every training pair (two-pass, f64).
    let pairs: f64 = train.iter().map(|e| e.options.len() as f64).sum::<f64>().max(1.0);
    let mut mean = vec![0.0f64; f];
    for e in train {
        for &o in &e.options {
            features(&vecs[e.state], &vecs[o], &mut buf);
            for i in 0..f {
                mean[i] += buf[i] as f64;
            }
        }
    }
    mean.iter_mut().for_each(|m| *m /= pairs);
    let mut var = vec![0.0f64; f];
    for e in train {
        for &o in &e.options {
            features(&vecs[e.state], &vecs[o], &mut buf);
            for i in 0..f {
                let d = buf[i] as f64 - mean[i];
                var[i] += d * d;
            }
        }
    }
    let inv: Vec<f32> = var.iter().map(|v| { let sd = (v / pairs).sqrt(); if sd > 1e-9 { (1.0 / sd) as f32 } else { 0.0 } }).collect();

    // `w` lives in standardized space; scores use w ⊙ inv on raw features (the
    // mean is the same for every option of a decision and cancels).
    let mut w = vec![0.0f32; f];
    let (mut m, mut v) = (vec![0.0f32; f], vec![0.0f32; f]);
    let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
    let n = train.len().max(1) as f64;
    let max_k = train.iter().map(|e| e.options.len()).max().unwrap_or(0);
    let mut xs = vec![0.0f32; max_k * f];
    let mut grad = vec![0.0f64; f];
    for epoch in 0..cfg.epochs {
        let ws: Vec<f32> = w.iter().zip(&inv).map(|(a, b)| a * b).collect();
        grad.iter_mut().for_each(|g| *g = 0.0);
        for e in train {
            let k = e.options.len();
            let mut scores = Vec::with_capacity(k);
            for (j, &o) in e.options.iter().enumerate() {
                let x = &mut xs[j * f..(j + 1) * f];
                features(&vecs[e.state], &vecs[o], x);
                scores.push(dot(&ws, x));
            }
            let p = softmax(&scores);
            for j in 0..k {
                let g = p[j] - (j == e.label) as u8 as f64;
                let x = &xs[j * f..(j + 1) * f];
                for i in 0..f {
                    grad[i] += g * x[i] as f64;
                }
            }
        }
        let t = (epoch + 1) as i32;
        let (c1, c2) = (1.0 - b1.powi(t), 1.0 - b2.powi(t));
        for i in 0..f {
            let g = (grad[i] / n) as f32 * inv[i] + cfg.l2 * w[i];
            m[i] = b1 * m[i] + (1.0 - b1) * g;
            v[i] = b2 * v[i] + (1.0 - b2) * g * g;
            w[i] -= cfg.lr * (m[i] / c1) / ((v[i] / c2).sqrt() + eps);
        }
    }
    w.iter().zip(&inv).map(|(a, b)| a * b).collect()
}

pub fn weights_sha256(w: &[f32]) -> String {
    hex::encode(Sha256::digest(crate::memory_index::f32_to_bytes(w)))
}

/// Accuracy and mean top probability over `set`.
pub fn evaluate(w: &[f32], vecs: &[Vec<f32>], set: &[Example]) -> (f64, f64) {
    if set.is_empty() {
        return (0.0, 0.0);
    }
    let (mut right, mut conf) = (0usize, 0.0f64);
    for e in set {
        let opts: Vec<&[f32]> = e.options.iter().map(|&o| vecs[o].as_slice()).collect();
        let p = predict_probs(w, &vecs[e.state], &opts);
        let top = argmax(&p);
        right += (top == e.label) as usize;
        conf += p[top];
    }
    (right as f64 / set.len() as f64, conf / set.len() as f64)
}

/// The untrained baseline: pick the option most similar to the state.
pub fn cosine_accuracy(vecs: &[Vec<f32>], set: &[Example]) -> f64 {
    if set.is_empty() {
        return 0.0;
    }
    let right = set
        .iter()
        .filter(|e| {
            let sims: Vec<f64> = e.options.iter().map(|&o| dot(&vecs[e.state], &vecs[o])).collect();
            argmax(&sims) == e.label
        })
        .count();
    right as f64 / set.len() as f64
}

pub fn argmax(p: &[f64]) -> usize {
    // First maximum on ties: deterministic.
    let mut best = 0;
    for i in 1..p.len() {
        if p[i] > p[best] {
            best = i;
        }
    }
    best
}

// ── serving ─────────────────────────────────────────────────────────────────

/// A stored version, decoded.
pub struct Model {
    pub id: String,
    pub version: i64,
    pub embed_model: String,
    pub dim: usize,
    pub weights: Vec<f32>,
    /// `training::trained_ids_blob` of the version's training split.
    pub trained_ids: Vec<u8>,
}

/// The latest version for a kind, cached by artifact id.
pub fn latest_model(conn: &Connection, kind: &str) -> rusqlite::Result<Option<Arc<Model>>> {
    model(conn, kind, None)
}

/// A pinned version (`Some(n)`) or the newest, cached by artifact id.
pub fn model(conn: &Connection, kind: &str, pin: Option<i64>) -> rusqlite::Result<Option<Arc<Model>>> {
    static CACHE: Mutex<Option<HashMap<String, Arc<Model>>>> = Mutex::new(None);
    let Some((id, _)) = training::find(conn, NAME, kind, pin)? else { return Ok(None) };
    if let Some(m) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|c| c.get(&id).cloned())) {
        return Ok(Some(m));
    }
    let a = training::load(conn, &id)?;
    let weights = crate::memory_index::bytes_to_f32(a.weights.as_deref().unwrap_or_default());
    let dim = a.embed_dim.unwrap_or(0);
    if dim == 0 || weights.len() != n_features(dim) {
        tracing::warn!(kind, id, "decision head.embed: malformed artifact");
        return Ok(None);
    }
    let m = Arc::new(Model { id: id.clone(), version: a.version, embed_model: a.embed_model.unwrap_or_default(), dim, weights, trained_ids: a.trained_ids });
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(id, m.clone());
    }
    Ok(Some(m))
}

pub struct EmbedHead;

impl Head for EmbedHead {
    fn name(&self) -> &'static str {
        NAME
    }
    fn applies(&self, _kind: &str) -> bool {
        true
    }
    fn ready(&self, conn: &Connection, kind: &str) -> bool {
        training::latest(conn, NAME, kind).ok().flatten().is_some()
    }
    fn embeds(&self) -> bool {
        true
    }
    fn stored_versions(&self) -> bool {
        true
    }
    fn needs_text(&self) -> bool {
        true
    }
    fn current_version(&self, conn: &Connection, kind: &str) -> String {
        training::latest(conn, NAME, kind).ok().flatten().map(|(_, v)| v.to_string()).unwrap_or_default()
    }
    fn predict(&self, conn: &Connection, input: &HeadInput<'_>) -> rusqlite::Result<Option<HeadAnswer>> {
        let Some(v) = input.vectors else { return Ok(None) };
        let Some(m) = model(conn, input.kind, input.pin)? else { return Ok(None) };
        if !compatible(&m, v) || v.options.len() != input.ids.len() {
            return Ok(None);
        }
        // Leave-one-out: never credited for a decision it was fit on.
        if input.decision_id.is_some_and(|id| training::trained_on(&m.trained_ids, id)) {
            return Ok(None);
        }
        let opts: Vec<&[f32]> = v.options.iter().map(Vec::as_slice).collect();
        let probs = predict_probs(&m.weights, &v.state, &opts);
        Ok(Some(HeadAnswer { choice: argmax(&probs), probs }))
    }
}

/// Vectors from the model (and dimension) the head was trained on, only.
pub fn compatible(m: &Model, v: &Vectors) -> bool {
    v.model == m.embed_model && v.state.len() == m.dim && v.options.iter().all(|o| o.len() == m.dim)
}
