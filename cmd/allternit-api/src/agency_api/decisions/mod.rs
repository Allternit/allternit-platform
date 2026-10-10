//! Decision Runtime (Agency Kernel, Living Agent Architecture): one typed
//! decision over caller-supplied options, answered in about 100 ms when the
//! local scorer is warm.
//!
//! `POST /v1/decisions` (alias `POST /v1/systemone`) takes a context, up to
//! 255 options and a decision kind, walks the backend chain (`backends`) and
//! escalates while the confidence is under the kind's threshold, all inside
//! `latency_budget_ms`. Every decision is stored (`store`); its outcome comes
//! back later through `PATCH /v1/decisions/:id`.
//!
//! The host re-checks that the chosen option is still valid before acting.
//! A decision is never authorization: policy runs first, always.

pub mod backends;
pub mod store;

use super::{ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Extension, Json,
};
use backends::{Answer, DecisionBackend, Query};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MAX_OPTIONS: usize = 255;
const MAX_CONTEXT_BYTES: usize = 64 * 1024;
const MAX_IMAGE_BYTES: usize = 6 * 1024 * 1024;
const DEFAULT_BUDGET_MS: u64 = 10_000;
const MAX_BUDGET_MS: u64 = 120_000;
/// Below this much budget left, a backend is not started.
const MIN_SLICE_MS: u64 = 5;

#[derive(Deserialize)]
#[serde(untagged)]
enum ContextIn {
    Text(String),
    Parts { text: String, image: Option<String> },
}

#[derive(Deserialize)]
struct OptionIn {
    id: String,
    #[serde(alias = "label")]
    text: String,
}

#[derive(Deserialize)]
struct DecisionIn {
    #[serde(alias = "state")]
    context: ContextIn,
    #[serde(alias = "candidates")]
    options: Vec<OptionIn>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    allow_abstain: bool,
    #[serde(default, alias = "max_latency_ms")]
    latency_budget_ms: Option<u64>,
    /// Override the configured chain (names from `backends::backend`).
    #[serde(default)]
    backends: Option<Vec<String>>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    task: Option<String>,
}

#[derive(Deserialize)]
pub struct OutcomeIn {
    status: String,
    /// The option id that was right, when known (oracle, human, verifier).
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

/// Per-kind confidence thresholds: built-in defaults, overridden by the JSON
/// object in `ALLTERNIT_DECISIONS_THRESHOLDS` (`{"default": 0.6, "safety": 0.95}`).
pub fn threshold(kind: &str) -> f64 {
    let mut t: HashMap<String, f64> =
        [("default", 0.6), ("verify", 0.8), ("safety", 0.9), ("escalate", 0.7)].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    if let Some(over) = std::env::var("ALLTERNIT_DECISIONS_THRESHOLDS").ok().and_then(|s| serde_json::from_str::<HashMap<String, f64>>(&s).ok()) {
        t.extend(over);
    }
    t.get(kind).or_else(|| t.get("default")).copied().unwrap_or(0.6).clamp(0.0, 1.0)
}

fn bad(msg: impl Into<String>, param: &str, rid: &RequestId) -> ApiError {
    ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", msg, rid).param(Some(param.into()))
}

fn validate(d: &DecisionIn, rid: &RequestId) -> Result<(), ApiError> {
    let (text, image) = match &d.context {
        ContextIn::Text(t) => (t, None),
        ContextIn::Parts { text, image } => (text, image.as_ref()),
    };
    if text.len() > MAX_CONTEXT_BYTES {
        return Err(bad(format!("context is over {MAX_CONTEXT_BYTES} bytes"), "context", rid));
    }
    if let Some(img) = image {
        if !img.starts_with("data:image/") || img.len() > MAX_IMAGE_BYTES {
            return Err(bad("context.image must be a data:image/... URL under 6 MB", "context.image", rid));
        }
    }
    if !(2..=MAX_OPTIONS).contains(&d.options.len()) {
        return Err(bad(format!("options must have 2..{MAX_OPTIONS} entries"), "options", rid));
    }
    let mut seen = HashSet::new();
    for o in &d.options {
        if o.id.is_empty() || o.id.len() > 128 || o.id == backends::ABSTAIN {
            return Err(bad("each option needs an id of 1-128 characters", "options.id", rid));
        }
        if o.text.trim().is_empty() || o.text.len() > 2000 {
            return Err(bad(format!("option {} needs text of 1-2000 bytes", o.id), "options.text", rid));
        }
        if !seen.insert(o.id.as_str()) {
            return Err(bad(format!("duplicate option id {}", o.id), "options.id", rid));
        }
    }
    if let Some(k) = &d.kind {
        if k.is_empty() || k.len() > 64 || !k.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
            return Err(bad("kind must be 1-64 characters of [A-Za-z0-9_.-]", "kind", rid));
        }
    }
    if let Some(names) = &d.backends {
        if let Some(bad_name) = names.iter().find(|n| backends::backend(n).is_none()) {
            return Err(bad(format!("unknown backend {bad_name}"), "backends", rid));
        }
    }
    Ok(())
}

/// The routed result before it is stored.
pub struct Routed {
    pub choice: Option<usize>,
    pub probs: Vec<f64>,
    pub confidence: f64,
    pub abstained: bool,
    pub escalated: bool,
    pub backend: Option<&'static str>,
    pub attempts: Vec<Value>,
}

/// Walk the chain: skip what is off or blind to the image, escalate while
/// under `thr`, never past `budget`. The last answer wins (a later backend is
/// the stronger one). Abstain when the final confidence is under `thr` and
/// the caller allows it.
pub async fn route(chain: &[Box<dyn DecisionBackend>], q: &Query<'_>, thr: f64, budget: Duration) -> Routed {
    let started = Instant::now();
    let mut attempts = Vec::new();
    let mut best: Option<(&'static str, Answer)> = None;
    let mut escalated = false;
    for b in chain {
        let name = b.name();
        if !b.enabled() {
            continue;
        }
        if q.image.is_some() && !b.vision() {
            attempts.push(json!({ "backend": name, "skipped": "cannot see the image" }));
            continue;
        }
        let left = budget.saturating_sub(started.elapsed());
        if left < Duration::from_millis(MIN_SLICE_MS) {
            attempts.push(json!({ "backend": name, "skipped": "latency budget spent" }));
            break;
        }
        // Trying another backend after an answer is an escalation.
        escalated |= best.is_some();
        let t = Instant::now();
        let out = tokio::time::timeout(left, b.decide(q)).await;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        match out {
            Err(_) => attempts.push(json!({ "backend": name, "latency_ms": ms, "error": "latency budget spent" })),
            Ok(Err(e)) => attempts.push(json!({ "backend": name, "latency_ms": ms, "error": e })),
            Ok(Ok(a)) => {
                let conf = if a.abstain { 0.0 } else { a.probs.iter().cloned().fold(0.0, f64::max) };
                attempts.push(json!({ "backend": name, "latency_ms": ms, "confidence": conf, "abstain": a.abstain, "detail": a.detail }));
                let confident = !a.abstain && conf >= thr;
                best = Some((name, a));
                if confident {
                    break;
                }
            }
        }
    }
    let Some((name, a)) = best else {
        return Routed { choice: None, probs: vec![], confidence: 0.0, abstained: true, escalated, backend: None, attempts };
    };
    let top = (0..a.probs.len()).max_by(|&i, &j| a.probs[i].total_cmp(&a.probs[j]));
    let confidence = if a.abstain { 0.0 } else { top.map(|i| a.probs[i]).unwrap_or(0.0) };
    let abstained = a.abstain || (q.allow_abstain && confidence < thr);
    Routed {
        choice: if a.abstain || abstained { None } else { top },
        probs: a.probs,
        confidence,
        abstained,
        escalated,
        backend: Some(name),
        attempts,
    }
}

pub async fn create_decision(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    body: axum::body::Bytes,
) -> ApiResult {
    let d: DecisionIn = serde_json::from_slice(&body).map_err(|e| bad(format!("invalid decision request: {e}"), "body", &rid))?;
    decide(&st, &user, &rid, &d).await.map(|v| Json(v).into_response())
}

/// In-process entry for hosts inside allternit-api (computer use's
/// `run_subtask`): the same validation, routing and store as
/// `POST /v1/decisions`, without the HTTP hop. `body` is the request JSON.
pub async fn decide_value(st: &Arc<AppState>, user: &AuthUser, body: Value) -> Result<Value, String> {
    let rid = RequestId(super::store::new_id("req"));
    let d: DecisionIn = serde_json::from_value(body).map_err(|e| format!("invalid decision request: {e}"))?;
    decide(st, user, &rid, &d).await.map_err(|e| e.body["error"]["message"].as_str().unwrap_or("decision failed").to_string())
}

/// In-process outcome write, the same as `PATCH /v1/decisions/:id`.
pub fn record_outcome(st: &AppState, user: &AuthUser, id: &str, status: &str, label: Option<&str>, detail: Option<&str>) -> Result<(), String> {
    if !OUTCOMES.contains(&status) {
        return Err(format!("status must be one of {}", OUTCOMES.join(", ")));
    }
    let detail: Option<String> = detail.map(|d| d.chars().take(4000).collect());
    match store::set_outcome(&st.db, id, &user.user_id, status, label, detail.as_deref(), &super::store::now()) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!("decision {id} not found")),
        Err(e) => Err(e.to_string()),
    }
}

async fn decide(st: &Arc<AppState>, user: &AuthUser, rid: &RequestId, d: &DecisionIn) -> Result<Value, ApiError> {
    let started = Instant::now();
    validate(d, rid)?;
    let (context, image) = match &d.context {
        ContextIn::Text(t) => (t.as_str(), None),
        ContextIn::Parts { text, image } => (text.as_str(), image.as_deref()),
    };
    let kind = d.kind.as_deref().unwrap_or("choice");
    let q = Query {
        kind,
        context,
        question: d.question.as_deref().filter(|s| !s.trim().is_empty()),
        image,
        ids: d.options.iter().map(|o| o.id.as_str()).collect(),
        texts: d.options.iter().map(|o| o.text.as_str()).collect(),
        allow_abstain: d.allow_abstain,
    };
    let chain: Vec<Box<dyn DecisionBackend>> = match &d.backends {
        Some(names) => names.iter().filter_map(|n| backends::backend(n)).collect(),
        None => backends::chain(),
    };
    let budget = Duration::from_millis(d.latency_budget_ms.unwrap_or(DEFAULT_BUDGET_MS).clamp(1, MAX_BUDGET_MS));
    let thr = threshold(kind);
    let r = route(&chain, &q, thr, budget).await;

    if r.backend.is_none() && !d.allow_abstain {
        return Err(ApiError::new(503, "SYSTEM", "ERR_DECISION_UNAVAILABLE", "no decision backend answered within the latency budget", rid)
            .with_detail(json!({ "attempts": r.attempts })));
    }
    // With allow_abstain, nothing answering is an abstention, still recorded.
    Ok(finish(st, user, rid, d, kind, context, image.is_some(), &q, r, thr, started))
}

#[allow(clippy::too_many_arguments)]
fn finish(
    st: &Arc<AppState>,
    user: &AuthUser,
    rid: &RequestId,
    d: &DecisionIn,
    kind: &str,
    context: &str,
    has_image: bool,
    q: &Query<'_>,
    r: Routed,
    thr: f64,
    started: Instant,
) -> Value {
    let backend = r.backend.unwrap_or("none");
    let id = super::store::new_id("dec");
    let created_at = super::store::now();
    let probs: serde_json::Map<String, Value> = q.ids.iter().zip(&r.probs).map(|(i, p)| (i.to_string(), json!(round4(*p)))).collect();
    let probs = Value::Object(probs);
    let choice = r.choice.map(|i| q.ids[i].to_string());
    let latency_ms = (started.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0;
    let options = json!(d.options.iter().map(|o| json!({ "id": o.id, "text": o.text })).collect::<Vec<_>>());
    let context_hash = hex::encode(Sha256::digest(context.as_bytes()));
    let attempts = json!(r.attempts);
    let row = store::Row {
        id: &id,
        owner: &user.user_id,
        created_at: &created_at,
        kind,
        session_id: d.session_id.as_deref(),
        task: d.task.as_deref(),
        context_hash: &context_hash,
        has_image,
        options: &options,
        choice: choice.as_deref(),
        probs: &probs,
        confidence: r.confidence,
        abstained: r.abstained,
        backend,
        latency_ms,
        attempts: &attempts,
    };
    // The decision is served even when it cannot be logged; the log line says so.
    if let Err(e) = store::insert(&st.db, &row) {
        tracing::warn!("decision store: {e}");
    }
    json!({
        "id": id,
        "object": "decision",
        "created_at": created_at,
        "kind": kind,
        "choice": choice,
        "probs": probs,
        "confidence": round4(r.confidence),
        "abstained": r.abstained,
        "escalated": r.escalated,
        "threshold": thr,
        "calibrated": false,
        "backend": backend,
        "latency_ms": latency_ms,
        "attempts": attempts,
        "request_id": rid.0,
    })
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

pub async fn get_decision(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    match store::get(&st.db, &id, &user.user_id) {
        Ok(Some(v)) => Ok(Json(v).into_response()),
        Ok(None) => Err(ApiError::not_found("decision", &rid)),
        Err(e) => Err(ApiError::internal(e, &rid)),
    }
}

pub const OUTCOMES: &[&str] = &["success", "failure", "error", "skipped"];

pub async fn patch_decision(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let o: OutcomeIn = serde_json::from_slice(&body).map_err(|e| bad(format!("invalid outcome: {e}"), "body", &rid))?;
    if !OUTCOMES.contains(&o.status.as_str()) {
        return Err(bad(format!("status must be one of {}", OUTCOMES.join(", ")), "status", &rid));
    }
    if o.detail.as_ref().is_some_and(|d| d.len() > 4000) || o.label.as_ref().is_some_and(|l| l.len() > 128) {
        return Err(bad("detail is limited to 4000 bytes and label to 128", "detail", &rid));
    }
    match store::set_outcome(&st.db, &id, &user.user_id, &o.status, o.label.as_deref(), o.detail.as_deref(), &super::store::now()) {
        Ok(Some(v)) => Ok(Json(v).into_response()),
        Ok(None) => Err(ApiError::not_found("decision", &rid)),
        Err(e) => Err(ApiError::internal(e, &rid)),
    }
}

#[cfg(test)]
mod tests;
